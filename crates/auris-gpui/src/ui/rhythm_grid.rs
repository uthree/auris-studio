//! The player's step buttons and the drummer's instrument-by-time grid.

use crate::app::AurisApp;
use crate::ui::widgets::{ButtonStyle, button};
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, IntoElement, div, prelude::*};

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
    pub(crate) fn rhythm_grid(&self, clip: ClipId, cx: &mut gpui::Context<Self>) -> AnyElement {
        let Ok(grid) = self.session.clip_rhythm_grid(clip) else {
            return div().into_any_element();
        };
        let theme = self.theme.clone();
        let steps = grid.rows.first().map_or(0, |row| row.steps.len());
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
            .min_w_0()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text)
                    .child(self.t(Key::PartRhythm)),
            )
            .child(
                div().flex().gap_1().min_w_0().child(labels).child(
                    div()
                        .id("rhythm-grid-scroll")
                        .flex_1()
                        .min_w_0()
                        .overflow_x_scroll()
                        .child(matrix),
                ),
            )
            .child(
                div()
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
    }
}
