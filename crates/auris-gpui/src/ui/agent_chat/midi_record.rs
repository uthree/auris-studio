//! Recording into a reference score: playing it on a MIDI keyboard, or singing it.
//!
//! The reference editor is a session of its own, so that nothing done to the score reaches the
//! song. It opens with its audio asleep, and only one session at a time may hold the hardware —
//! the output, the input and the MIDI keyboards, which on Windows one program at a time can open.
//! Recording therefore *lends* it all three: every project is put to sleep, this one included, the
//! editor wakes, records as the song editor does — the count-in, the click, the typing keyboard,
//! the MIDI keyboards and, with voice input on, the microphone — and hands the hardware back when
//! the take stops.
//!
//! The loan is watched by a task that outlives the draft, not owned by it: a draft can be dropped
//! mid-take (a conversation switched, an editor cancelled), and the hardware must still come back.
//! The task polls the editor's session while it records — a headless window has no repaint loop
//! of its own to do it — and when the take is over, or the editor gone, puts it to sleep and wakes
//! the project again.
//!
//! The take always merges into the part being edited: a reference is one clip a part, and a take
//! that became a clip of its own would be one the attachment never sees. A take recorded past the
//! clip's end is folded into it when it stops.

use super::*;
use auris_session::prelude::{ClipId, MidiClip, Ticks};
use auris_session::{NoteRecordMode, SessionError};

impl AurisApp {
    /// Whether the open reference editor is recording.
    #[cfg(test)]
    pub(crate) fn reference_recording(&self, cx: &gpui::App) -> bool {
        self.music_references()
            .midi_draft
            .as_ref()
            .and_then(|draft| draft.editor.as_ref())
            .is_some_and(|editor| editor.read(cx).app.read(cx).session.is_recording())
    }

    /// Starts recording into the part the reference editor shows, or stops the take running.
    pub(crate) fn toggle_reference_recording(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(editor) = self
            .music_references()
            .midi_draft
            .as_ref()
            .and_then(|draft| draft.editor.clone())
        else {
            return;
        };
        let child = editor.read(cx).app.clone();
        if child.read(cx).session.is_recording() {
            child.update(cx, |app, cx| {
                match app.session.stop_recording() {
                    Ok(_) => {}
                    Err(error) => log::warn!("the reference take did not land: {error}"),
                }
                app.session.stop();
                fold_takes(app, &editor_clips(&editor, cx));
                cx.notify();
            });
            // The task watching the loan sees the take is over and hands the hardware back.
            cx.notify();
            return;
        }
        if self.session.is_recording() {
            self.set_failed_status(self.t(Key::OtherProjectRecording).to_string());
            return;
        }
        if !self.release_all_audio(cx) {
            // Whatever was put to sleep before the refusal wakes again with this project.
            self.acquire_audio(cx);
            return;
        }
        let preferences = auris_session::Settings::load().audio;
        let midi = self.settings.midi.clone();
        let count_in = self.session.count_in_bars();
        let metronome = self.session.metronome();
        let started = child.update(cx, |app, cx| {
            let result = (|| {
                app.session.set_audio_preferences(preferences)?;
                app.session.set_audio_suspended(false)?;
                app.session.apply_midi_preferences(&midi);
                app.session.set_note_record_mode(NoteRecordMode::Merge);
                app.session.set_count_in_bars(count_in);
                app.session.set_metronome(metronome);
                app.session.set_midi_input(midi.enabled);
                app.session.start_recording(app.selected_track)?;
                if !app.session.is_playing() {
                    app.session.play();
                }
                Ok::<_, SessionError>(())
            })();
            if result.is_err() {
                app.session.set_midi_input(false);
                let _ = app.session.set_audio_suspended(true);
            }
            cx.notify();
            result
        });
        if let Err(error) = started {
            self.music_references_mut().attachment_error =
                Some(crate::i18n::error_text(&error, self.language()));
            self.acquire_audio(cx);
            return;
        }
        self.lent_audio = Some(child.downgrade());
        self.set_status(self.t(Key::RecordingStarted));
        self.watch_lent_audio(cx);
    }

    /// Polls the editor holding this project's hardware, and takes it back when its take ends.
    fn watch_lent_audio(&mut self, cx: &mut gpui::Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        let child = this.lent_audio.as_ref().and_then(|weak| weak.upgrade());
                        let recording = child.as_ref().is_some_and(|child| {
                            child.update(cx, |app, cx| {
                                app.session.poll();
                                cx.notify();
                                app.session.is_recording()
                            })
                        });
                        if recording {
                            cx.notify();
                            return false;
                        }
                        if let Some(child) = child {
                            child.update(cx, |app, _| {
                                app.session.stop();
                                app.session.set_midi_input(false);
                                let _ = app.session.set_audio_suspended(true);
                            });
                        }
                        this.lent_audio = None;
                        this.acquire_audio(cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }
}

/// The reference clips an editor shows, one a part.
fn editor_clips(
    editor: &gpui::Entity<crate::ui::reference_piano_roll::ReferencePianoRoll>,
    cx: &gpui::App,
) -> Vec<ClipId> {
    editor.read(cx).clips.clone()
}

/// Moves the notes of every other clip on a part's track into the part's own clip.
///
/// A merging take lands in the clip it overlaps; one recorded wholly past the end of it lands in a
/// clip of its own, which the attachment would never read.
fn fold_takes(app: &mut AurisApp, parts: &[ClipId]) {
    for &part in parts {
        let Some(track) = app.session.track_of_clip(part) else {
            continue;
        };
        let Some(base) = app.session.midi_clip(part).cloned() else {
            continue;
        };
        let strays: Vec<MidiClip> = app
            .project()
            .track(track)
            .and_then(|track| track.kind.note_clips())
            .map(|clips| {
                clips
                    .iter()
                    .filter(|clip| clip.id != part && !parts.contains(&clip.id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if strays.is_empty() {
            continue;
        }
        let mut notes = base.notes.clone();
        let mut end = base.start + base.length;
        for stray in &strays {
            let offset = stray.start.raw() - base.start.raw();
            notes.extend(stray.notes.iter().filter_map(|note| {
                let start = note.start.raw() + offset;
                (start >= 0).then(|| {
                    let mut note = note.clone();
                    note.start = Ticks(start);
                    note
                })
            }));
            end = end.max(stray.start + stray.length);
        }
        notes.sort_by_key(|note| (note.start, note.pitch));
        let ids: Vec<ClipId> = strays.iter().map(|clip| clip.id).collect();
        let result = app
            .session
            .resize_clip(part, end)
            .and_then(|()| app.session.replace_notes(part, notes))
            .and_then(|()| app.session.remove_clips(&ids));
        if let Err(error) = result {
            log::warn!("could not fold a reference take into its part: {error}");
        }
    }
}
