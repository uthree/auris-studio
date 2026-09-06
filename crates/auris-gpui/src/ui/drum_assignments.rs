//! Manual musical-role assignments for a drum track's future generated clips.

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*};

use crate::app::AurisApp;
use crate::ui::drums::role_key;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{ButtonStyle, button};

/// A MIDI address, validated without rounding or clamping to a different sound.
pub(super) fn parse_assignment_note(text: &str) -> Option<u8> {
    text.trim().parse::<u8>().ok().filter(|note| *note <= 127)
}

impl AurisApp {
    /// Editable role-to-note assignments, independent of any acoustic analysis in progress.
    pub(crate) fn drum_assignment_rows(
        &self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(map) = self.session.drum_assignments(track) else {
            return Vec::new();
        };
        let theme = &self.theme;
        let mut rows = vec![
            div()
                .mt_2()
                .text_xs()
                .text_color(theme.text)
                .child(self.t(Key::DrumAssignments))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::DrumAssignmentsHint))
                .into_any_element(),
        ];
        for role in DrumRole::ALL {
            let assigned = map.voices.get(&role).copied();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(role_key(role))),
                    )
                    .child(
                        button(
                            ("drum-assignment-note", role as usize),
                            assigned.map_or_else(
                                || self.t(Key::DrumAddAssignment).to_string(),
                                |note| format!("MIDI {note}"),
                            ),
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.open_drum_assignment_prompt(track, role);
                                cx.notify();
                            }),
                        )
                        .debug_selector(move || format!("drum-assignment-note-{}", role as usize)),
                    )
                    .when(assigned.is_some(), |row| {
                        row.child(
                            button(
                                ("drum-assignment-clear", role as usize),
                                self.t(Key::DrumClearAssignment),
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(move |this, _, _, cx| {
                                    match this.session.set_drum_assignment(track, role, None) {
                                        Ok(_) => {
                                            this.set_status(this.t(Key::EditSetDrumAssignment))
                                        }
                                        Err(error) => this.set_failed_status(
                                            this.failure(Key::EditSetDrumAssignment, &error),
                                        ),
                                    }
                                    cx.notify();
                                }),
                            )
                            .debug_selector(move || {
                                format!("drum-assignment-clear-{}", role as usize)
                            }),
                        )
                    })
                    .into_any_element(),
            );
        }
        rows
    }

    /// Opens the exact MIDI-address field for an existing or unassigned role.
    pub(crate) fn open_drum_assignment_prompt(&mut self, track: TrackId, role: DrumRole) {
        let Some(map) = self.session.drum_assignments(track) else {
            return;
        };
        let value = map.voices.get(&role).map(u8::to_string).unwrap_or_default();
        self.open_prompt(Prompt::new(
            format!("{} · {}", self.t(role_key(role)), self.t(Key::DrumMidiNote)),
            PromptTarget::DrumAssignment { track, role },
            value,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_address_must_be_an_exact_midi_integer() {
        assert_eq!(parse_assignment_note("0"), Some(0));
        assert_eq!(parse_assignment_note("127"), Some(127));
        assert_eq!(parse_assignment_note(" 73 "), Some(73));
        for text in ["", "128", "256", "-1", "36.5", "C2", "NaN"] {
            assert_eq!(parse_assignment_note(text), None, "{text}");
        }
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::harness::{self, click, paint};

    #[gpui::test]
    fn assignments_can_be_changed_cleared_and_added_by_typing(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        let (track, clip, original) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this.session.add_default_drum_track("Kit").unwrap();
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    ClipRecipe::new(ClipPreset::Drums, 12),
                )
                .unwrap();
            let original = this.session.midi_clip(clip).unwrap().clone();
            this.select_clip(None);
            this.selected_track = Some(track);
            this.show_panel(crate::dock::Panel::Inspector);
            (track, clip, original)
        });
        paint(&app, cx);
        click("drum-assignment-note-0", cx);
        paint(&app, cx);
        cx.simulate_input("73");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                73
            );
            assert_eq!(this.session.midi_clip(clip), Some(&original));
        });

        click("drum-assignment-note-0", cx);
        paint(&app, cx);
        cx.simulate_input("128");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let prompt = this
                .prompt
                .as_ref()
                .expect("invalid addresses stay editable");
            assert_eq!(prompt.field().unwrap().content(), "128");
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                73
            );
        });
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("127");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                127
            )
        });

        click("drum-assignment-clear-0", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                !this
                    .session
                    .drum_assignments(track)
                    .unwrap()
                    .voices
                    .contains_key(&DrumRole::Kick)
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                127
            )
        });
        click("drum-assignment-clear-0", cx);
        paint(&app, cx);
        click("drum-assignment-note-0", cx);
        paint(&app, cx);
        cx.simulate_input("0");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let map = this.session.drum_assignments(track).unwrap();
            assert_eq!(map.voices[&DrumRole::Kick], 0);
            assert_eq!(map.voices[&DrumRole::Snare], 38);
            assert_eq!(
                this.session.midi_clip(clip),
                Some(&original),
                "all manual edits leave existing notes and recipes intact"
            );
        });
    }

    #[gpui::test]
    fn cancelling_a_new_assignment_preserves_the_unassigned_role(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        let track = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this.session.add_default_drum_track("Kit").unwrap();
            this.session
                .set_drum_assignment(track, DrumRole::Kick, None)
                .unwrap();
            this.selected_track = Some(track);
            this.show_panel(crate::dock::Panel::Inspector);
            track
        });
        paint(&app, cx);
        click("drum-assignment-note-0", cx);
        paint(&app, cx);
        cx.simulate_input("55");
        cx.simulate_keystrokes("escape");
        app.update(cx, |this, cx| {
            assert!(this.prompt.is_none());
            assert!(
                !this
                    .session
                    .drum_assignments(track)
                    .unwrap()
                    .voices
                    .contains_key(&DrumRole::Kick)
            );
            let melodic = this.session.add_default_instrument_track("Keys").unwrap();
            assert!(this.drum_assignment_rows(melodic, cx).is_empty());
        });
    }
}
