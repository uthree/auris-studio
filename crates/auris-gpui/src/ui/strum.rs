//! Controls for a continuous strumming hand and partial upstrokes.
use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::performance::{PerformDial, rank};
use crate::ui::widgets::disclosure;
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, prelude::*};

/// Reads a configurable strum or an older attack-clock stroke.
pub(crate) fn strum_settings(stack: &[NoteTransform]) -> Strum {
    stack
        .iter()
        .find_map(|t| match t {
            NoteTransform::Strum { settings } => Some(settings.clone()),
            NoteTransform::Stroke {
                spread_ms,
                direction,
            } => Some(Strum {
                spread_ms: *spread_ms,
                direction: *direction,
                clock: StrumClock::Attacks,
                ..Strum::default()
            }),
            _ => None,
        })
        .unwrap_or_default()
}

/// Replaces a strumming stage in its original stack position.
pub(crate) fn with_strum_settings(stack: &[NoteTransform], settings: Strum) -> Vec<NoteTransform> {
    let mut out = stack.to_vec();
    let stage = NoteTransform::Strum { settings };
    let at = out.iter().position(|t| {
        matches!(
            t,
            NoteTransform::Strum { .. } | NoteTransform::Stroke { .. }
        )
    });
    if let Some(at) = at {
        out[at] = stage;
    } else {
        let at = out
            .iter()
            .position(|t| rank(t) > rank(&stage))
            .unwrap_or(out.len());
        out.insert(at, stage);
    }
    out
}

fn clock_key(clock: StrumClock) -> Key {
    match clock {
        StrumClock::Attacks => Key::PerformStrumAttacks,
        StrumClock::Eighths => Key::PerformStrumEighths,
        StrumClock::Sixteenths => Key::PerformStrumSixteenths,
    }
}

impl AurisApp {
    pub(crate) fn strum_detail_rows(
        &self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = &self.theme;
        let mut rows = vec![
            disclosure(
                "perform-strum-details",
                self.t(Key::PerformStrumSettings),
                self.performance_details[1],
                theme,
                cx.listener(|this, _, _, cx| {
                    this.performance_details[1] = !this.performance_details[1];
                    cx.notify();
                }),
            )
            .min_w_0()
            .overflow_hidden()
            .into_any_element(),
        ];
        if !self.performance_details[1] {
            return rows;
        }
        let settings = strum_settings(stack);
        for dial in [PerformDial::StrumUpVelocity, PerformDial::StrumLowAccent] {
            rows.push(self.performance_slider(clip, dial, stack, cx));
        }
        rows.push(
            self.picker_row(
                "perform-strum-clock",
                Key::PerformStrumClock,
                self.t(clock_key(settings.clock)).to_string(),
                Self::opens_menu(cx, move |this, at| {
                    let settings =
                        strum_settings(this.session.clip_transforms(clip).unwrap_or(&[]));
                    let mut menu = ContextMenu::new(at, this.t(Key::PerformStrumClock));
                    for clock in [
                        StrumClock::Attacks,
                        StrumClock::Eighths,
                        StrumClock::Sixteenths,
                    ] {
                        menu = menu.toggle(
                            this.t(clock_key(clock)),
                            MenuCommand::SetStrumSettings {
                                clip,
                                settings: Strum {
                                    clock,
                                    ..settings.clone()
                                },
                            },
                            clock == settings.clock,
                        );
                    }
                    menu
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.picker_row(
                "perform-strum-up-notes",
                Key::PerformStrumUpNotes,
                if settings.up_notes == 0 {
                    self.t(Key::PerformStrumAllNotes).to_string()
                } else {
                    settings.up_notes.to_string()
                },
                Self::opens_menu(cx, move |this, at| {
                    let settings =
                        strum_settings(this.session.clip_transforms(clip).unwrap_or(&[]));
                    let mut menu = ContextMenu::new(at, this.t(Key::PerformStrumUpNotes));
                    for up_notes in [0, 1, 2, 3, 4] {
                        menu = menu.toggle(
                            if up_notes == 0 {
                                this.t(Key::PerformStrumAllNotes).to_string()
                            } else {
                                up_notes.to_string()
                            },
                            MenuCommand::SetStrumSettings {
                                clip,
                                settings: Strum {
                                    up_notes,
                                    ..settings.clone()
                                },
                            },
                            up_notes == settings.up_notes,
                        );
                    }
                    menu
                }),
            )
            .into_any_element(),
        );
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{choose, click, paint, resize, with_a_clip};
    use gpui::{TestAppContext, px, size};

    #[gpui::test]
    fn partial_upstroke_and_clock_pickers_refresh_the_read_only_preview(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            for start in [0, 240] {
                for pitch in [48, 60, 64, 67] {
                    this.session
                        .add_note(clip, Note::new(pitch, Ticks(start), Ticks(120)))
                        .unwrap();
                }
            }
            this.open_clip_in_editor(clip);
            this.set_perform_dial(clip, PerformDial::Stroke, 0.2);
        });
        resize(&app, cx, size(px(1920.0), px(1800.0)));
        click("score-performed", cx);
        click("perform-strum-details", cx);
        paint(&app, cx);
        click("perform-strum-up-notes", cx);
        paint(&app, cx);
        choose(
            &app,
            cx,
            &MenuCommand::SetStrumSettings {
                clip,
                settings: Strum {
                    up_notes: 2,
                    ..Strum::default()
                },
            },
        );
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap().len(), 6);
            assert_eq!(this.session.midi_clip(clip).unwrap().notes.len(), 8);
        });
        click("perform-strum-clock", cx);
        paint(&app, cx);
        choose(
            &app,
            cx,
            &MenuCommand::SetStrumSettings {
                clip,
                settings: Strum {
                    up_notes: 2,
                    clock: StrumClock::Eighths,
                    ..Strum::default()
                },
            },
        );
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap().len(), 8)
        });
        app.update(cx, |this, _| this.undo());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap().len(), 6)
        });
    }
}
