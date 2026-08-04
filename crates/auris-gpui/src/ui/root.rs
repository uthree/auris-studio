//! The window's root layout, global pointer handling and action dispatch.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    Axis, Context, IntoElement, MouseMoveEvent, MouseUpEvent, Render, Window, div, prelude::*, px,
    relative,
};

use crate::actions;
use crate::app::{AurisApp, Drag, EditorTab};
use crate::gestures::past_drag_threshold;
use crate::theme::Theme;
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

        let theme = self.theme.clone();
        let panels = self.panels.clone();
        let menu_bar = self.render_menu_bar(window, cx);
        let transport = self.render_transport(window, cx);
        let arrangement = self.render_arrangement(window, cx);
        let editor = panels.editor_visible.then(|| match self.editor {
            EditorTab::PianoRoll => self.render_piano_roll(window, cx),
            EditorTab::Mixer => self.render_mixer(window, cx).into_any_element(),
        });
        let library = panels
            .library_visible
            .then(|| self.render_library(window, cx));
        let library_splitter = panels.library_visible.then(|| {
            splitter(
                "split-library",
                Axis::Vertical,
                &theme,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    let start_width = this.panels.library_width;
                    this.begin_drag(Drag::ResizeLibrary {
                        start_x: event.position.x,
                        start_width,
                    });
                }),
            )
        });
        let plugin_window = self.render_plugin_window(window.viewport_size(), cx);
        let inspector = panels
            .inspector_visible
            .then(|| self.render_inspector(window, cx));
        let editor_splitter = panels.editor_visible.then(|| {
            splitter(
                "split-editor",
                Axis::Horizontal,
                &theme,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    let start_height = this.panels.editor_height;
                    this.begin_drag(Drag::ResizeEditor {
                        start_y: event.position.y,
                        start_height,
                    });
                }),
            )
        });
        let inspector_splitter = panels.inspector_visible.then(|| {
            splitter(
                "split-inspector",
                Axis::Vertical,
                &theme,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    let start_width = this.panels.inspector_width;
                    this.begin_drag(Drag::ResizeInspector {
                        start_x: event.position.x,
                        start_width,
                    });
                }),
            )
        });
        let status = self.render_status_bar();
        let export_overlay = self.render_export_overlay(cx);
        let prompt = self.render_prompt(cx);
        let palette = self.render_palette(cx);
        let menu = self.render_context_menu(window, cx);

        div()
            .id("root")
            // While something is being typed into the application's own bindings must not fire:
            // `i` has to type an `i`, not toggle the inspector. Every binding is scoped to
            // `Auris`, so switching the context here disables the lot in one move and lets the
            // keystrokes through to the text input handler.
            .key_context(if self.prompt.is_some() || self.palette.is_some() {
                "AurisPrompt"
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
            .on_action(cx.listener(Self::on_new_project))
            .on_action(cx.listener(Self::on_open_project))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_compose_song))
            .on_action(cx.listener(Self::on_save_project))
            .on_action(cx.listener(Self::on_save_project_as))
            .on_action(cx.listener(Self::on_import_audio))
            .on_action(cx.listener(Self::on_import_soundfont))
            .on_action(cx.listener(Self::on_collect_assets))
            .on_action(cx.listener(Self::on_export_audio))
            .on_action(cx.listener(Self::on_add_instrument_track))
            .on_action(cx.listener(Self::on_add_audio_track))
            .on_action(cx.listener(Self::on_delete_track))
            .on_action(cx.listener(Self::on_delete_selection))
            .on_action(cx.listener(Self::on_next_tool))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_panic_stop))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_toggle_library))
            .on_action(cx.listener(Self::on_toggle_inspector))
            .on_action(cx.listener(Self::on_toggle_editor))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_open_command_palette))
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
                    // The library leads, then the arrangement, then the inspector. Only the
                    // middle column carries `flex_1().min_w_0()`, so a narrowing window takes
                    // the timeline and never a panel — a panel that shrank would move every
                    // hit test in it out from under the pointer.
                    .children(library.map(|library| {
                        div()
                            .w(panels.library_width)
                            .flex_shrink_0()
                            .flex()
                            .child(library)
                    }))
                    .children(library_splitter)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(arrangement)
                            .children(editor_splitter)
                            .children(editor.map(|editor| {
                                div()
                                    .h(panels.editor_height)
                                    .flex_shrink_0()
                                    .flex()
                                    .child(editor)
                            })),
                    )
                    .children(inspector_splitter)
                    .children(inspector.map(|inspector| {
                        div()
                            .w(panels.inspector_width)
                            .flex_shrink_0()
                            .flex()
                            .child(inspector)
                    })),
            )
            .child(status)
            .children(export_overlay)
            // These come last so they paint — and are hit-tested — above the panels. The plugin
            // editor sits below the menu because a right-click inside it opens one.
            .children(prompt)
            .children(palette)
            .children(plugin_window)
            .children(menu)
    }
}

impl AurisApp {
    fn render_status_bar(&self) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let audio = self.session.audio_status();
        let engine = if audio.running {
            format!("{:.0} Hz · {} ch", audio.sample_rate, audio.channels)
        } else {
            self.t(Key::EngineSilent).to_string()
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(crate::theme::Metrics::STATUS_HEIGHT)
            .px_2()
            .bg(theme.surface_raised)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.text_muted)
            .child(
                div()
                    .flex_1()
                    .truncate()
                    // A failure was reported in the same pale grey as the sample rate beside it,
                    // in a row a user has no reason to be looking at.
                    .when(self.status_failed, |this| this.text_color(theme.danger))
                    .child(self.status.clone()),
            )
            .child(self.window_title())
            .child(engine)
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

    fn on_mouse_move(
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
            Drag::ClipResize { clip } => {
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.snap_unless_held(self.timeline.x_to_tick(x), event.modifiers);
                let _ = self.session.resize_clip(clip, tick);
            }
            Drag::NoteMove {
                clip,
                origin_tick,
                origin_pitch,
                ref origins,
            } => {
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
            Drag::NoteResize { clip, index } => {
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
            Drag::PartDial {
                clip,
                dial,
                start_fraction,
                start_x,
            } => {
                let delta = f32::from(event.position.x - start_x);
                self.drag_dial(clip, dial, start_fraction, delta);
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
            Drag::Tempo { start_bpm, start_x } => {
                // Half a beat per pixel would be unusable; 0.25 BPM/px lets a short drag cover
                // the musically interesting range while still landing on exact values.
                let delta = f64::from(f32::from(event.position.x - start_x)) * 0.25;
                self.session.set_bpm(start_bpm + delta);
            }
            Drag::MovePluginWindow { grab_offset } => {
                if let Some(window) = self.plugin_window.as_mut() {
                    window.anchor = gpui::point(
                        event.position.x - grab_offset.x,
                        event.position.y - grab_offset.y,
                    );
                }
            }
            Drag::ResizeLibrary {
                start_x,
                start_width,
            } => self.resize_library(start_width, event.position.x - start_x),
            Drag::ResizeInspector {
                start_x,
                start_width,
            } => self.resize_inspector(start_width, event.position.x - start_x),
            Drag::ResizeEditor {
                start_y,
                start_height,
            } => self.resize_editor(start_height, event.position.y - start_y),
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

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
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
        {
            cx.stop_propagation();
            cx.notify();
        }
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

    fn on_toggle_library(
        &mut self,
        _: &actions::ToggleLibrary,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_library();
        cx.notify();
    }

    fn on_toggle_inspector(
        &mut self,
        _: &actions::ToggleInspector,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_inspector();
        cx.notify();
    }

    fn on_toggle_editor(
        &mut self,
        _: &actions::ToggleEditor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_editor();
        cx.notify();
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
