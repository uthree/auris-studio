//! Independent expression controls and MIDI groove reference selection.
use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::performance::{PerformDial, rank};
use crate::ui::widgets::disclosure;
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, prelude::*};

pub(crate) fn expression_settings(stack: &[NoteTransform], seed: u64) -> Expression {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Expression { settings } => Some(settings.clone()),
            NoteTransform::Humanize { amount, seed } => Some(Expression {
                timing: *amount,
                velocity: *amount,
                seed: *seed,
                ..Expression::default()
            }),
            _ => None,
        })
        .unwrap_or(Expression {
            seed,
            ..Expression::default()
        })
}

pub(crate) fn with_expression_settings(
    stack: &[NoteTransform],
    settings: Expression,
) -> Vec<NoteTransform> {
    let mut out = stack.to_vec();
    let stage = NoteTransform::Expression { settings };
    if let Some(at) = out.iter().position(|t| {
        matches!(
            t,
            NoteTransform::Expression { .. } | NoteTransform::Humanize { .. }
        )
    }) {
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

impl AurisApp {
    pub(crate) fn expression_detail_rows(
        &self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = &self.theme;
        let mut rows = vec![
            disclosure(
                "perform-expression-details",
                self.t(Key::PerformExpressionSettings),
                self.performance_details[2],
                theme,
                cx.listener(|this, _, _, cx| {
                    this.performance_details[2] = !this.performance_details[2];
                    cx.notify();
                }),
            )
            .min_w_0()
            .overflow_hidden()
            .into_any_element(),
        ];
        if !self.performance_details[2] {
            return rows;
        }
        for dial in [
            PerformDial::ExpressionTiming,
            PerformDial::ExpressionVelocity,
            PerformDial::PhraseSwell,
            PerformDial::BeatAccent,
            PerformDial::EnsembleShared,
            PerformDial::PerformanceDelay,
        ] {
            rows.push(self.performance_slider(clip, dial, stack, cx));
        }
        let settings = expression_settings(stack, clip.0);
        rows.push(
            self.picker_row(
                "perform-ensemble-group",
                Key::PerformEnsembleGroup,
                settings.group.to_string(),
                Self::opens_menu(cx, move |this, at| {
                    let settings = expression_settings(
                        this.session.clip_transforms(clip).unwrap_or(&[]),
                        clip.0,
                    );
                    let mut menu = ContextMenu::new(at, this.t(Key::PerformEnsembleGroup));
                    for group in 1..=4 {
                        menu = menu.toggle(
                            group.to_string(),
                            MenuCommand::SetExpressionSettings {
                                clip,
                                settings: Expression {
                                    group,
                                    ..settings.clone()
                                },
                            },
                            group == settings.group,
                        );
                    }
                    menu
                }),
            )
            .into_any_element(),
        );
        let reference = stack.iter().find_map(|stage| match stage {
            NoteTransform::Groove { template, .. } => Some(template.name.clone()),
            _ => None,
        });
        rows.push(
            self.picker_row(
                "perform-groove-reference",
                Key::PerformGrooveReference,
                reference
                    .clone()
                    .unwrap_or_else(|| self.t(Key::PerformGrooveChoose).to_string()),
                Self::opens_menu(cx, move |this, at| {
                    let mut menu = ContextMenu::new(at, this.t(Key::PerformGrooveReference));
                    for track in &this.session.project().tracks {
                        if let Some(clips) = track.kind.note_clips() {
                            for source in clips.iter().filter(|source| source.id != clip) {
                                menu = menu.item(
                                    format!("{} / {}", track.name, source.name),
                                    MenuCommand::CaptureGroove {
                                        target: clip,
                                        source: source.id,
                                    },
                                );
                            }
                        }
                    }
                    menu
                }),
            )
            .into_any_element(),
        );
        if reference.is_some() {
            for dial in [PerformDial::GrooveTiming, PerformDial::GrooveVelocity] {
                rows.push(self.performance_slider(clip, dial, stack, cx));
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{choose, click, paint, resize, with_a_clip};
    use crate::ui::performance::{dial_fraction, with_dial};
    use gpui::{TestAppContext, px, size};

    #[gpui::test]
    fn reference_picker_and_independent_controls_update_the_performance_layer(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let reference = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .add_note(clip, Note::new(60, Ticks(480), Ticks(120)))
                .unwrap();
            let reference = this
                .session
                .add_midi_clip(track, "Reference", Ticks(7680), Ticks(1920))
                .unwrap();
            this.session
                .add_note(reference, Note::new(72, Ticks(540), Ticks(120)))
                .unwrap();
            this.open_clip_in_editor(clip);
            reference
        });
        resize(&app, cx, size(px(1920.0), px(2200.0)));
        click("score-performed", cx);
        click("perform-expression-details", cx);
        paint(&app, cx);
        click("perform-groove-reference", cx);
        paint(&app, cx);
        choose(
            &app,
            cx,
            &MenuCommand::CaptureGroove {
                target: clip,
                source: reference,
            },
        );
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap()[0].start, Ticks(540));
            assert_eq!(
                this.session.midi_clip(clip).unwrap().notes[0].start,
                Ticks(480)
            );
        });
        let at = cx.debug_bounds("perform-dial-13").unwrap().center();
        crate::harness::press(cx, at);
        let end = gpui::point(at.x + px(120.0), at.y);
        crate::harness::drag_to(cx, end);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap()[0].start, Ticks(540));
            assert_ne!(
                this.score_preview_notes().unwrap()[0].velocity,
                this.session.midi_clip(clip).unwrap().notes[0].velocity
            );
        });
        crate::harness::release(cx, end);
        app.update(cx, |this, _| {
            this.set_perform_dial(clip, PerformDial::GrooveTiming, 0.0)
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap()[0].start, Ticks(480))
        });
    }

    #[test]
    fn independent_axes_keep_the_seed_and_master_preserves_their_ratio() {
        let original = vec![NoteTransform::Humanize {
            amount: 0.8,
            seed: 91,
        }];
        let timing = with_dial(&original, PerformDial::ExpressionTiming, 0.4, 10);
        let settings = expression_settings(&timing, 10);
        assert_eq!(
            (settings.timing, settings.velocity, settings.seed),
            (0.4, 0.8, 91)
        );
        let master = with_dial(&timing, PerformDial::Humanize, 0.4, 10);
        assert_eq!(dial_fraction(&master, PerformDial::ExpressionTiming), 0.2);
        assert_eq!(dial_fraction(&master, PerformDial::ExpressionVelocity), 0.4);
        let muted = with_dial(&master, PerformDial::Humanize, 0.0, 10);
        assert!(matches!(&muted[0], NoteTransform::Expression { .. }));
    }
}
