//! The player's step buttons and the drummer's instrument-by-time grid.

use crate::app::AurisApp;
use crate::ui::widgets::{ButtonStyle, button};
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, IntoElement, div, prelude::*};
use gpui::{
    Bounds, Context, FocusHandle, Render, ScrollHandle, Subscription, WeakEntity, Window,
    WindowBounds, WindowOptions, px, size,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

/// A resizable editor for one generated clip, sharing the main window's session.
pub(crate) struct RhythmWindow {
    app: WeakEntity<AurisApp>,
    clip: ClipId,
    focus: FocusHandle,
    horizontal: ScrollHandle,
    ready: bool,
    _observe: Subscription,
}

impl Render for RhythmWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // open_window draws synchronously while the owner is still leased by its click handler.
        // Read the shared session only after that handler has returned.
        if !self.ready {
            self.ready = true;
            cx.notify();
            return div().into_any_element();
        }
        let Some(app) = self.app.upgrade() else {
            window.remove_window();
            return div().into_any_element();
        };
        if app.read(cx).rhythm_window.map(gpui::AnyWindowHandle::from)
            != Some(window.window_handle())
        {
            window.remove_window();
            return div().into_any_element();
        }
        let (theme, title, content) = app.update(cx, |app, cx| {
            (
                app.theme.clone(),
                format!(
                    "{} — {}",
                    app.t(Key::PartRhythm),
                    app.session
                        .midi_clip(self.clip)
                        .map_or("", |clip| clip.name.as_str())
                ),
                app.rhythm_grid(self.clip, &self.horizontal, cx),
            )
        });
        window.set_window_title(&title);
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus);
        }
        let bar = crate::titlebar::titlebar(window, &theme)
            .child(
                crate::titlebar::drag_region("rhythm-title")
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .child(div().min_w_0().truncate().child(title)),
            )
            .child(crate::titlebar::controls(window, &theme, |_, window, _| {
                window.remove_window()
            }));
        div()
            .id("rhythm-window")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.key == "escape" {
                    window.remove_window();
                    cx.stop_propagation();
                } else if key.modifiers.secondary() && key.key == "z" {
                    let _ = this.app.update(cx, |app, cx| {
                        if key.modifiers.shift {
                            app.redo()
                        } else {
                            app.undo()
                        };
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            }))
            .child(bar)
            .child(
                div()
                    .id("rhythm-window-body")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .overflow_y_scroll()
                    .p_3()
                    .child(content),
            )
            .into_any_element()
    }
}

gpui::actions!(
    rhythm_grid,
    [
        /// Activates the focused rhythm step or automatic-generation button.
        Activate
    ]
);

pub(crate) fn key_bindings() -> [gpui::KeyBinding; 2] {
    [
        gpui::KeyBinding::new("space", Activate, Some("AurisRhythmCell")),
        gpui::KeyBinding::new("enter", Activate, Some("AurisRhythmCell")),
    ]
}

impl AurisApp {
    /// Opens or retargets the shared rhythm editor for a generated clip.
    pub(crate) fn open_rhythm_window(&mut self, clip: ClipId, cx: &mut Context<Self>) {
        if let Some(handle) = self.rhythm_window
            && handle
                .update(cx, |view, window, cx| {
                    if view.clip != clip {
                        view.clip = clip;
                        view.horizontal = ScrollHandle::new();
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
        {
            return;
        }
        let app = cx.entity();
        let weak = app.downgrade();
        let bounds = Bounds::centered(None, size(px(760.0), px(460.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(crate::titlebar::options(self.t(Key::PartRhythm))),
                window_min_size: Some(size(px(480.0), px(260.0))),
                focus: true,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| RhythmWindow {
                    app: weak,
                    clip,
                    focus: cx.focus_handle(),
                    horizontal: ScrollHandle::new(),
                    ready: false,
                    _observe: cx.observe(&app, |_, _, cx| cx.notify()),
                })
            },
        );
        match opened {
            Ok(handle) => self.rhythm_window = Some(handle),
            Err(error) => self.set_status(error.to_string()),
        }
    }

    fn rhythm_grid(
        &self,
        clip: ClipId,
        horizontal: &ScrollHandle,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let Ok(grid) = self.session.clip_rhythm_grid(clip) else {
            return div()
                .child(self.t(Key::PartRhythmUnavailable))
                .into_any_element();
        };
        let theme = self.theme.clone();
        let steps = grid.rows.first().map_or(0, |row| row.steps.len());
        let matrix_height = gpui::rems(1.5 + grid.rows.len() as f32 * 3.25 + 1.0);
        let mut labels = div()
            .flex()
            .flex_col()
            .gap_1()
            .w_24()
            .flex_shrink_0()
            .child(div().h_6());
        let mut matrix = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(gpui::rems(steps as f32 * 1.75));
        let mut header = div().flex().gap_1().items_center().h_6();
        for step in 0..steps {
            header = header.child(
                div()
                    .w_6()
                    .flex_shrink_0()
                    .text_xs()
                    .text_center()
                    .text_color(theme.text_muted)
                    .child(if step % grid.steps_per_beat == 0 {
                        (step / grid.steps_per_beat + 1).to_string()
                    } else {
                        String::new()
                    }),
            );
        }
        matrix = matrix.child(header);
        for (row_index, row) in grid.rows.into_iter().enumerate() {
            let name = row.role.map_or(self.t(Key::PartRhythm), |role| {
                self.t(crate::ui::drums::role_key(role))
            });
            let reset_voice = row.voice.clone();
            let reset_key = reset_voice.clone();
            let label = div()
                .w_24()
                .h_12()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text)
                        .truncate()
                        .child(name),
                )
                .child(if row.editable {
                    button(
                        ("rhythm-auto", row_index),
                        self.t(Key::PartRhythmAuto),
                        ButtonStyle::Normal,
                        row.automatic,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            match this.session.reset_clip_rhythm_row(clip, &reset_voice) {
                                Ok(_) => this.forget_rewritten_notes(clip),
                                Err(error) => {
                                    this.set_failed_status(this.failure(Key::PartRhythm, &error))
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .focusable()
                    .tab_stop(true)
                    .key_context("AurisRhythmCell")
                    .focus(|style| style.border_color(theme.text))
                    .on_action(cx.listener(move |this, _: &Activate, _, cx| {
                        this.change_rhythm_cell(clip, &reset_key, None, cx);
                    }))
                    .into_any_element()
                } else {
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::PartRhythmFixed))
                        .into_any_element()
                });
            labels = labels.child(label);
            let mut line = div().flex().gap_1().items_center().h_12();
            for (step, active) in row.steps.into_iter().enumerate() {
                let voice = row.voice.clone();
                let key_voice = voice.clone();
                let cell = if row.editable {
                    button(
                        ("rhythm-cell", row_index * steps + step),
                        if active { "●" } else { "" },
                        ButtonStyle::Normal,
                        active,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            match this.session.toggle_clip_rhythm_step(clip, &voice, step) {
                                Ok(_) => this.forget_rewritten_notes(clip),
                                Err(error) => {
                                    this.set_failed_status(this.failure(Key::PartRhythm, &error))
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .w_6()
                    .h_6()
                    .p_0()
                    .rounded_none()
                    .flex_shrink_0()
                    .cursor_default()
                    .focusable()
                    .tab_stop(true)
                    .key_context("AurisRhythmCell")
                    .focus(|style| style.border_color(theme.text).border_2())
                    .tooltip(crate::ui::tooltip::keyed_tip(
                        format!("{name} · {}", step + 1),
                        "Space",
                        &theme,
                    ))
                    .on_action(cx.listener(move |this, _: &Activate, _, cx| {
                        this.change_rhythm_cell(clip, &key_voice, Some(step), cx);
                    }))
                    .border_color(if step % grid.steps_per_beat == 0 {
                        theme.text_muted
                    } else {
                        theme.border
                    })
                    .into_any_element()
                } else {
                    div()
                        .w_6()
                        .h_6()
                        .flex_shrink_0()
                        .border_1()
                        .border_color(theme.border)
                        .bg(if active {
                            theme.accent
                        } else {
                            theme.surface_raised
                        })
                        .into_any_element()
                };
                line = line.child(cell);
            }
            matrix = matrix.child(line);
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .min_w_0()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text)
                    .child(self.t(Key::PartRhythm)),
            )
            .child(
                div().flex().gap_1().w_full().min_w_0().child(labels).child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h(matrix_height)
                        .child(
                            div()
                                .id("rhythm-grid-scroll")
                                .debug_selector(|| "rhythm-grid-scroll".to_string())
                                .w_full()
                                .min_w_0()
                                .h(matrix_height)
                                .pb_4()
                                .overflow_x_scroll()
                                .track_scroll(horizontal)
                                .child(matrix),
                        )
                        .child(div().absolute().inset_0().child(
                            Scrollbar::horizontal(horizontal).scrollbar_show(ScrollbarShow::Always),
                        )),
                ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(if steps == 0 {
                        Key::PartRhythmNoVoices
                    } else {
                        Key::PartRhythmGridHint
                    })),
            )
            .into_any_element()
    }

    fn change_rhythm_cell(
        &mut self,
        clip: ClipId,
        voice: &str,
        step: Option<usize>,
        cx: &mut gpui::Context<Self>,
    ) {
        let result = match step {
            Some(step) => self.session.toggle_clip_rhythm_step(clip, voice, step),
            None => self.session.reset_clip_rhythm_row(clip, voice),
        };
        match result {
            Ok(_) => self.forget_rewritten_notes(clip),
            Err(error) => self.set_failed_status(self.failure(Key::PartRhythm, &error)),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint};
    use gpui::TestAppContext;

    #[gpui::test]
    fn clicking_a_square_edits_rhythm_and_auto_restores_it(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (clip, original, was_on) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            let track = this.session.add_default_instrument_track("Player").unwrap();
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    ClipRecipe::new(ClipPreset::Arp, 1),
                )
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            (
                clip,
                this.session.midi_clip(clip).unwrap().clone(),
                this.session.clip_rhythm_grid(clip).unwrap().rows[0].steps[0],
            )
        });
        paint(&app, cx);
        click("part-rhythm-edit", cx);
        let handle = app.read_with(cx, |app, _| app.rhythm_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        click("rhythm-cell-0", cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            let grid = this.session.clip_rhythm_grid(clip).unwrap();
            assert!(!grid.rows[0].automatic);
            assert_eq!(grid.rows[0].steps[0], !was_on);
        });
        // Space activates the focused square, rather than the transport.
        cx.simulate_keystrokes("space");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.clip_rhythm_grid(clip).unwrap().rows[0].steps[0],
                was_on
            );
        });
        paint(&app, cx);
        click("rhythm-auto-0", cx);
        app.update(cx, |this, _| {
            assert_eq!(this.session.midi_clip(clip), Some(&original));
            this.session.undo().unwrap();
            assert!(!this.session.clip_rhythm_grid(clip).unwrap().rows[0].automatic);
        });
        app.update(cx, |app, cx| app.open_rhythm_window(clip, cx));
        assert!(
            app.read_with(cx, |app, _| gpui::AnyWindowHandle::from(
                app.rhythm_window.unwrap()
            )) == gpui::AnyWindowHandle::from(handle),
            "reopening reuses the existing editor"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(handle.update(cx, |_, _, _| ()).is_err());
        app.update(cx, |app, cx| app.open_rhythm_window(clip, cx));
        cx.run_until_parked();
        let reopened = app.read_with(cx, |app, _| app.rhythm_window.unwrap());
        assert!(
            reopened
                .update(cx, |view, _, _| assert_eq!(view.clip, clip))
                .is_ok()
        );
    }

    #[gpui::test]
    fn drum_rows_edit_independently_in_the_visible_grid(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (clip, original) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this
                .session
                .add_drum_track("Kit", "auris.synth.drumkit")
                .unwrap();
            for (role, note) in [
                (DrumRole::Kick, 36),
                (DrumRole::Snare, 38),
                (DrumRole::ClosedHat, 42),
            ] {
                this.session
                    .set_drum_assignment(track, role, Some(note))
                    .unwrap();
            }
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    ClipRecipe::new(ClipPreset::Drums, 1),
                )
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            (clip, this.session.midi_clip(clip).unwrap().clone())
        });
        paint(&app, cx);
        click("part-rhythm-edit", cx);
        let handle = app.read_with(cx, |app, _| app.rhythm_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        click("rhythm-cell-16", cx);
        app.update(cx, |this, _| {
            let grid = this.session.clip_rhythm_grid(clip).unwrap();
            assert!(grid.rows[0].automatic);
            assert!(!grid.rows[1].automatic);
            assert!(grid.rows[2].automatic);
            let others = |midi: &MidiClip| {
                midi.notes
                    .iter()
                    .filter(|note| note.pitch != 38)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                others(this.session.midi_clip(clip).unwrap()),
                others(&original)
            );
            this.session.undo().unwrap();
            assert_eq!(this.session.midi_clip(clip), Some(&original));
        });
        crate::harness::resize(&app, cx, size(px(480.0), px(300.0)));
        let was_on = app.read_with(cx, |app, _| {
            app.session.clip_rhythm_grid(clip).unwrap().rows[1].steps[15]
        });
        let viewport = cx.debug_bounds("rhythm-grid-scroll").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-10000.0), px(0.0))),
            ..Default::default()
        });
        paint(&app, cx);
        click("rhythm-cell-31", cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.session.clip_rhythm_grid(clip).unwrap().rows[1].steps[15],
                !was_on
            )
        });
        cx.simulate_keystrokes("secondary-z");
        app.read_with(cx, |app, _| {
            assert_eq!(app.session.midi_clip(clip), Some(&original))
        });
        cx.simulate_keystrokes("secondary-shift-z");
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.session.clip_rhythm_grid(clip).unwrap().rows[1].steps[15],
                !was_on
            )
        });
        app.update(cx, |app, cx| {
            app.new_project();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            handle.update(cx, |_, _, _| ()).is_err(),
            "a new document closes the old editor"
        );
    }
}
