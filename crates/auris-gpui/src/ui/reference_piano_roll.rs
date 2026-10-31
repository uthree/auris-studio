//! The normal piano roll hosted on an independent conversational score.

use auris_session::{SessionError, midi_reference::MidiReference, prelude::*};
use gpui::{Context, Entity, IntoElement, Render, Window, prelude::*};

use crate::{
    app::{AurisApp, Pane},
    dock::PanelLayout,
};

pub(crate) struct ReferencePianoRoll {
    pub(crate) app: Entity<AurisApp>,
    pub(crate) clips: Vec<ClipId>,
}

impl ReferencePianoRoll {
    pub(crate) fn new(
        score: &MidiReference,
        cx: &mut gpui::App,
    ) -> Result<Entity<Self>, SessionError> {
        let app = cx.new(AurisApp::new_reference_editor);
        let clips = app.update(cx, |app, _| {
            app.panels = PanelLayout::default();
            let origin = Ticks(score.source_origin_ticks);
            app.session
                .set_tempo_at(Ticks::ZERO, score.tempo_map.bpm_at(origin));
            app.session
                .set_signature_at(Ticks::ZERO, score.signatures.signature_at(origin));
            for point in score.tempo_map.points().iter().filter(|p| p.tick > origin) {
                app.session.set_tempo_point(point.tick - origin, point.bpm);
            }
            for point in score.signatures.points().iter().filter(|p| p.tick > origin) {
                app.session
                    .set_signature_point(point.tick - origin, point.signature);
            }
            let mut clips = Vec::with_capacity(score.tracks.len());
            for track in &score.tracks {
                let id = app.session.add_singer_track(&track.name);
                let length = track
                    .notes
                    .iter()
                    .map(Note::end)
                    .max()
                    .unwrap_or(Ticks::ZERO)
                    .max(Ticks(8 * TICKS_PER_QUARTER));
                let clip = app
                    .session
                    .add_midi_clip(id, &track.name, Ticks::ZERO, length)?;
                app.session.replace_notes(clip, track.notes.clone())?;
                clips.push(clip);
            }
            app.selected_clip = clips.first().copied();
            app.selected_track = app
                .selected_clip
                .and_then(|clip| app.project().track_of_clip(clip));
            app.selected_clips = app.selected_clip.into_iter().collect();
            app.last_pane = Pane::PianoRoll;
            if let Some(note) = score.tracks[0].notes.first() {
                app.pitch.top_pitch = note.pitch.saturating_add(6).min(127);
                app.timeline.scroll_ticks = note.start;
            } else {
                app.pitch.top_pitch = 72;
            }
            app.session.forget_history();
            Ok::<_, SessionError>(clips)
        })?;
        Ok(cx.new(|cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            Self { app, clips }
        }))
    }
}

impl Render for ReferencePianoRoll {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            app.claim_event_window(window);
            app.reconcile_focus(window, cx);
            let roll = app.render_piano_roll(window, cx);
            let overlays = app.render_document_overlays(window, cx);
            // Shared overlays use window coordinates and must escape the composer's scroll clip.
            let overlays = (!overlays.is_empty()).then(|| {
                gpui::deferred(
                    gpui::anchored().position(gpui::Point::default()).child(
                        gpui::div()
                            .relative()
                            .w(window.viewport_size().width)
                            .h(window.viewport_size().height)
                            .children(overlays),
                    ),
                )
            });
            app.reference_editor_input(cx)
                .id("reference-piano-roll")
                .debug_selector(|| "reference-piano-roll".into())
                .key_context(app.window_context())
                .track_focus(&app.focus)
                .relative()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    app.pane(Pane::PianoRoll, window, cx)
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(roll),
                )
                .children(overlays)
                .into_any_element()
        })
    }
}
