//! The window's root layout, global pointer handling and action dispatch.

use auris_core::time::Ticks;
use gpui::{
    Context, IntoElement, MouseMoveEvent, MouseUpEvent, Render, Window, div, prelude::*, px,
    relative,
};

use crate::actions;
use crate::app::{AurisApp, Drag, EditorTab};
use crate::theme::Theme;

impl Render for AurisApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pointer coordinates arrive in window space, and several hit tests need to know how
        // tall the window is to locate the bottom panel; record it once per frame.
        self.viewport_height = window.viewport_size().height;
        self.arrangement_width = window.viewport_size().width
            - crate::theme::Metrics::TRACK_HEADER_WIDTH
            - crate::theme::Metrics::INSPECTOR_WIDTH;

        // Keep the playhead on screen while the transport rolls, but never fight the user's
        // own scrolling when it is stopped.
        if self.is_playing() {
            let playhead = self.playhead_ticks();
            let width = self.arrangement_width;
            self.timeline.scroll_to_reveal(playhead, width);
        }

        let theme = self.theme.clone();
        let transport = self.render_transport(window, cx);
        let arrangement = self.render_arrangement(window, cx);
        let editor = match self.editor {
            EditorTab::PianoRoll => self.render_piano_roll(window, cx),
            EditorTab::Mixer => self.render_mixer(window, cx).into_any_element(),
        };
        let inspector = self.render_inspector(window, cx);
        let status = self.render_status_bar();
        let export_overlay = self.render_export_overlay(cx);

        div()
            .id("root")
            .key_context("Auris")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("Helvetica")
            .text_sm()
            .on_action(cx.listener(Self::on_toggle_play))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_return_to_zero))
            .on_action(cx.listener(Self::on_toggle_loop))
            .on_action(cx.listener(Self::on_new_project))
            .on_action(cx.listener(Self::on_open_project))
            .on_action(cx.listener(Self::on_save_project))
            .on_action(cx.listener(Self::on_save_project_as))
            .on_action(cx.listener(Self::on_import_audio))
            .on_action(cx.listener(Self::on_export_audio))
            .on_action(cx.listener(Self::on_add_instrument_track))
            .on_action(cx.listener(Self::on_add_audio_track))
            .on_action(cx.listener(Self::on_delete_track))
            .on_action(cx.listener(Self::on_delete_selection))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_panic_stop))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            // Drags are tracked on the root so they keep working after the pointer leaves the
            // control that started them, which is what makes a fader usable.
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(transport)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(arrangement)
                            .child(
                                div()
                                    .h(crate::theme::Metrics::EDITOR_HEIGHT)
                                    .flex_shrink_0()
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .flex()
                                    .child(editor),
                            ),
                    )
                    .child(inspector),
            )
            .child(status)
            .children(export_overlay)
    }
}

impl AurisApp {
    fn render_status_bar(&self) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let engine = if self.engine.is_running() {
            format!(
                "{:.0} Hz · {} ch · {} frames",
                self.engine.sample_rate(),
                self.engine.channel_count(),
                self.engine.max_block()
            )
        } else {
            "silent".to_string()
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
            .child(div().flex_1().truncate().child(self.status.clone()))
            .child(self.window_title())
            .child(engine)
    }

    /// A modal-ish overlay shown while an export runs.
    fn render_export_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let export = self.export.as_ref()?;
        let theme = self.theme.clone();
        let fraction = export.fraction();
        let finished = export.result.is_some();
        let message = match &export.result {
            Some(Ok(summary)) => summary.clone(),
            Some(Err(error)) => error.clone(),
            None => format!("Rendering {}…", export.path.display()),
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(420.0))
                        .p_4()
                        .rounded_lg()
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(div().text_sm().text_color(theme.text).child("Export"))
                        .child(div().text_xs().text_color(theme.text_muted).child(message))
                        .child(
                            div()
                                .h(px(6.0))
                                .w_full()
                                .rounded_sm()
                                .overflow_hidden()
                                .bg(theme.surface_sunken)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(if finished { 1.0 } else { fraction }))
                                        .bg(theme.accent),
                                ),
                        )
                        .when(finished, |this| {
                            this.child(crate::ui::widgets::button(
                                "export-close",
                                "Close",
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
                let tick = self.snap(self.timeline.x_to_tick(x));
                self.seek(tick);
            }
            Drag::LoopRegion { anchor } => {
                let x = event.position.x - self.timeline_origin().x;
                let tick = self.snap(self.timeline.x_to_tick(x)).max_zero();
                let (start, end) = if tick < anchor {
                    (tick, anchor)
                } else {
                    (anchor, tick)
                };
                self.project.loop_region = Some((start, end));
                self.push_loop_to_engine();
            }
            Drag::ClipMove { clip, grab_offset } => {
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.timeline.x_to_tick(x);
                let start = self.snap(tick - grab_offset).max_zero();
                self.move_clip(clip, start);
            }
            Drag::ClipResize { clip } => {
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.snap(self.timeline.x_to_tick(x));
                self.resize_clip(clip, tick);
            }
            Drag::NoteMove {
                clip,
                origin_tick,
                origin_pitch,
                ref origins,
            } => {
                let origin = self.roll_origin(self.viewport_height);
                let tick = self.timeline.x_to_tick(event.position.x - origin.x);
                let pitch = self.pitch.y_to_pitch(event.position.y - origin.y);
                let Some(clip_start) = self.project.midi_clip(clip).map(|(_, c)| c.start) else {
                    return;
                };
                let delta_ticks = self.snap(tick - clip_start) - self.snap(origin_tick);
                let delta_pitch = pitch as i32 - origin_pitch as i32;
                self.move_notes(clip, origins, delta_ticks, delta_pitch);
            }
            Drag::NoteResize { clip, index } => {
                let origin = self.roll_origin(self.viewport_height);
                let tick = self.timeline.x_to_tick(event.position.x - origin.x);
                let Some(clip_start) = self.project.midi_clip(clip).map(|(_, c)| c.start) else {
                    return;
                };
                let end = self.snap(tick - clip_start);
                self.resize_note(clip, index, end);
            }
            Drag::Param {
                target,
                start_value,
                start_x,
            } => {
                let delta = f32::from(event.position.x - start_x);
                self.drag_param(target, start_value, delta);
            }
            Drag::Tempo { start_bpm, start_x } => {
                // Half a beat per pixel would be unusable; 0.25 BPM/px lets a short drag cover
                // the musically interesting range while still landing on exact values.
                let delta = f64::from(f32::from(event.position.x - start_x)) * 0.25;
                self.set_bpm(start_bpm + delta);
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_audition();
        self.end_drag(window, cx);
        cx.notify();
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
        self.send(auris_engine::EngineCommand::Stop);
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
        self.new_project();
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
        self.send(auris_engine::EngineCommand::Panic);
        self.drag = None;
        self.set_status("Panic — all voices stopped");
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

    /// Moves a clip of either kind to a new start position.
    fn move_clip(&mut self, clip: auris_core::ClipId, start: Ticks) {
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.start = start;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.start = start;
        }
        self.dirty = true;
    }

    /// Drags a clip's right edge to `end`.
    fn resize_clip(&mut self, clip: auris_core::ClipId, end: Ticks) {
        let grid = self.project.grid;
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.length = (end - midi.start).max(grid);
            self.dirty = true;
            return;
        }
        // An audio clip's length lives in source frames, so convert the dragged tick back
        // through the tempo map rather than storing ticks.
        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            let start_seconds = tempo.ticks_to_seconds(audio.start).0;
            let end_seconds = tempo.ticks_to_seconds(end).0;
            let frames = ((end_seconds - start_seconds).max(0.0) * sample_rate) as u64;
            audio.length_frames = frames.max(1);
        }
        self.dirty = true;
    }

    /// Moves every note captured at the start of a drag.
    fn move_notes(
        &mut self,
        clip: auris_core::ClipId,
        origins: &[(usize, Ticks, u8)],
        delta_ticks: Ticks,
        delta_pitch: i32,
    ) {
        let grid = self.project.grid;
        if let Some(clip) = self.project.midi_clip_mut(clip) {
            for (index, start, pitch) in origins {
                if let Some(note) = clip.notes.get_mut(*index) {
                    note.start = (*start + delta_ticks).max_zero();
                    note.pitch = (*pitch as i32 + delta_pitch).clamp(0, 127) as u8;
                }
            }
            clip.fit_length_to_notes(grid);
        }
        self.dirty = true;
    }

    /// Drags a note's right edge to `end`, clip-relative.
    fn resize_note(&mut self, clip: auris_core::ClipId, index: usize, end: Ticks) {
        let grid = self.project.grid;
        if let Some(clip) = self.project.midi_clip_mut(clip) {
            if let Some(note) = clip.notes.get_mut(index) {
                note.length = (end - note.start).max(Ticks(grid.raw().max(1)));
            }
            clip.fit_length_to_notes(grid);
        }
        self.dirty = true;
    }
}
