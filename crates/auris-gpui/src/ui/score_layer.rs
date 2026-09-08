//! Editable source and a cached, read-only view of the notes sent to playback.

use std::sync::Arc;

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*};

use crate::app::{AurisApp, Pane};
use crate::theme::Metrics;
use crate::ui::widgets::{ButtonStyle, button};

/// The score shown by either MIDI editor.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum ScoreLayer {
    /// Stored notes, with all editing gestures available.
    #[default]
    Source,
    /// Playback notes, including generated articulations and loop passes.
    Performed,
}

/// One selected clip's performance, shared by paints until the document changes.
pub(crate) struct ScorePreview {
    revision: u64,
    clip: ClipId,
    notes: Arc<Vec<Note>>,
    bend: Arc<Vec<CurvePoint>>,
}

impl AurisApp {
    /// The cached playback bend, including every loop pass.
    pub(crate) fn score_preview_bend(&self) -> Arc<Vec<CurvePoint>> {
        self.score_preview
            .as_ref()
            .filter(|preview| {
                Some(preview.clip) == self.selected_clip
                    && preview.revision == self.session.revision()
            })
            .map(|preview| preview.bend.clone())
            .unwrap_or_default()
    }
    pub(crate) fn source_score(&self) -> bool {
        self.score_layer == ScoreLayer::Source
    }

    /// The last painted performance, for drum row hit testing and scrolling.
    pub(crate) fn score_preview_notes(&self) -> Option<&[Note]> {
        self.score_preview
            .as_ref()
            .filter(|preview| {
                Some(preview.clip) == self.selected_clip
                    && preview.revision == self.session.revision()
            })
            .map(|preview| preview.notes.as_slice())
    }

    /// Uses the same tempo, meter and loop expansion as the playback scheduler.
    pub(crate) fn score_notes(&mut self) -> Arc<Vec<Note>> {
        let Some(clip) = self.selected_midi_clip() else {
            return Arc::default();
        };
        if self.source_score() {
            return Arc::new(clip.notes.clone());
        }
        let revision = self.session.revision();
        if let Some(preview) = &self.score_preview
            && preview.revision == revision
            && preview.clip == clip.id
        {
            return preview.notes.clone();
        }
        let notes: Arc<Vec<Note>> = Arc::new(
            clip.sounding_notes_with_meter(
                self.project().tempo_map.bpm_at(clip.start),
                self.project().signatures.clone(),
            )
            .collect(),
        );
        let bend = Arc::new(
            clip.sounding_performance_curve_events(
                ClipCurve::Bend,
                auris_session::prelude::CURVE_STEP,
                &self.project().tempo_map,
                &self.project().signatures,
            )
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .map(|(at, value)| CurvePoint { at, value })
            .collect(),
        );
        self.score_preview = Some(ScorePreview {
            revision,
            clip: clip.id,
            notes: notes.clone(),
            bend,
        });
        notes
    }

    pub(crate) fn score_layer_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_2()
            .h(Metrics::PANEL_HEADER_HEIGHT)
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .children(
                [
                    (ScoreLayer::Source, "score-source", Key::ScoreSource),
                    (
                        ScoreLayer::Performed,
                        "score-performed",
                        Key::ScorePerformed,
                    ),
                ]
                .map(|(layer, id, label)| {
                    button(
                        id,
                        self.t(label),
                        ButtonStyle::Ghost,
                        self.score_layer == layer,
                        theme.accent_soft,
                        theme,
                        cx.listener(move |this, _, window, cx| {
                            if this.score_layer != layer {
                                this.end_drag(window, cx);
                                this.selected_notes.clear();
                                this.menu = None;
                                this.score_layer = layer;
                            }
                            this.focus_pane(Pane::PianoRoll, window);
                            cx.notify();
                        }),
                    )
                }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{
        click, click_at, creating, drag, drag_to, paint, press, release, right_press, roll_point,
        show_pitch, with_a_clip,
    };
    use crate::ui::context_menu::MenuCommand;
    use gpui::{TestAppContext, point, px};

    #[gpui::test]
    fn preview_tracks_live_dials_undo_loops_and_clip_selection(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let original = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            for pitch in [60, 64, 67] {
                this.session
                    .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                    .unwrap();
            }
            this.session
                .set_clip_loop(clip, Ticks::QUARTER * 8)
                .unwrap();
            this.session
                .set_clip_transforms(
                    clip,
                    vec![NoteTransform::Humanize {
                        amount: 0.8,
                        seed: 41,
                    }],
                )
                .unwrap();
            this.open_clip_in_editor(clip);
            this.session.midi_clip(clip).unwrap().notes.clone()
        });
        paint(&app, cx);
        click("score-performed", cx);
        paint(&app, cx);
        let initial = app.read_with(cx, |this, _| {
            let preview = &this.score_preview.as_ref().unwrap().notes;
            assert_eq!(preview.len(), 6, "the preview contains both loop passes");
            preview.clone()
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(Arc::ptr_eq(
                &initial,
                &this.score_preview.as_ref().unwrap().notes
            ));
        });
        let at = cx.debug_bounds("perform-dial-3").unwrap().center();
        press(cx, at);
        drag_to(cx, point(at.x + px(24.0), at.y));
        paint(&app, cx);
        let first_move = app.read_with(cx, |this, _| {
            assert!(this.drag.is_some(), "the slider has not been released");
            let notes = this.score_preview_notes().unwrap().to_vec();
            assert_ne!(notes, *initial);
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original);
            notes
        });
        drag_to(cx, point(at.x + px(48.0), at.y));
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let held = this.session.midi_clip(clip).unwrap();
            let expected: Vec<_> = held
                .sounding_notes_with_meter(
                    this.project().tempo_map.bpm_at(held.start),
                    this.project().signatures.clone(),
                )
                .collect();
            assert_eq!(this.score_preview_notes().unwrap(), expected);
            assert_ne!(
                expected, first_move,
                "every move refreshes, not only the first"
            );
        });
        release(cx, point(at.x + px(48.0), at.y));
        app.update(cx, |this, _| this.undo());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.score_preview_notes().unwrap(), initial.as_slice())
        });
        app.update(cx, |this, _| this.redo());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_ne!(this.score_preview_notes().unwrap(), initial.as_slice())
        });
        let before_tempo = app.update(cx, |this, _| {
            let before = this.score_preview_notes().unwrap().to_vec();
            this.session.set_tempo_at(Ticks::ZERO, 180.0);
            before
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_ne!(this.score_preview_notes().unwrap(), before_tempo);
            let held = this.session.midi_clip(clip).unwrap();
            let expected: Vec<_> = held
                .sounding_notes_with_meter(180.0, this.project().signatures.clone())
                .collect();
            assert_eq!(this.score_preview_notes().unwrap(), expected);
        });
        let other = app.update(cx, |this, _| {
            let other = this
                .session
                .add_midi_clip(track, "Other", Ticks::QUARTER * 8, Ticks::QUARTER * 4)
                .unwrap();
            this.session
                .add_note(other, Note::new(72, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.select_clip(Some(other));
            other
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let preview = this.score_preview.as_ref().unwrap();
            assert_eq!(preview.clip, other);
            assert_eq!(preview.notes.len(), 1);
            assert_eq!(preview.notes[0].pitch, 72);
        });
    }

    #[gpui::test]
    fn preview_is_read_only_and_source_remains_editable(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        let original = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.session
                .set_clip_transforms(clip, vec![NoteTransform::Brush { amount: 1.0 }])
                .unwrap();
            this.open_clip_in_editor(clip);
            this.selected_notes.insert(0);
            this.session.midi_clip(clip).unwrap().notes.clone()
        });
        paint(&app, cx);
        show_pitch(&app, cx, 60);
        click("score-performed", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.selected_notes.is_empty());
            assert!(this.score_preview_notes().unwrap().len() > original.len());
        });
        let from = roll_point(&app, cx, Ticks(TICKS_PER_QUARTER / 2), 60);
        let to = roll_point(&app, cx, Ticks::QUARTER * 2, 62);
        drag(cx, from, to);
        click_at(cx, to, creating());
        right_press(cx, from);
        cx.simulate_keystrokes("backspace");
        app.update(cx, |this, cx| {
            assert!(this.menu.is_none());
            for command in [
                MenuCommand::SelectAllNotes,
                MenuCommand::TransposeNotes(12),
                MenuCommand::DeleteNotes,
                MenuCommand::PasteNotes,
            ] {
                this.run_menu_command(command, cx);
            }
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original);
        });
        click("score-source", cx);
        paint(&app, cx);
        let from = roll_point(&app, cx, Ticks(TICKS_PER_QUARTER / 2), 60);
        let to = roll_point(&app, cx, Ticks(TICKS_PER_QUARTER * 3 / 2), 62);
        drag(cx, from, to);
        app.read_with(cx, |this, _| {
            let held = this.session.midi_clip(clip).unwrap();
            assert_eq!(held.notes.len(), 1);
            assert_eq!(held.notes[0].pitch, 62);
            assert_eq!(held.notes[0].start, Ticks::QUARTER);
            assert!(!held.transforms.is_empty());
        });
    }
}
