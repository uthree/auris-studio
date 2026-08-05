//! What the menus, buttons and key bindings do.
//!
//! These are thin: the document work happens in `auris-session`, and what remains here is the
//! desktop half — file dialogs, keeping the selection pointing at something that still exists,
//! moving long work off the main thread, and turning results into a status line.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::{Context, Window};

use crate::app::{AurisApp, Drag, ExportState};
use crate::i18n::{edit_key, error_text};
use crate::ui::drop::{DropAction, DropKind, DropOutcome, Dropped, drop_action};

impl AurisApp {
    /// Selects a track and points the piano roll at a clip that belongs to it.
    pub(crate) fn select_track(&mut self, track: TrackId) {
        if self.selected_track == Some(track) {
            return;
        }
        self.selected_track = Some(track);
        self.selected_notes.clear();
        // The clip selection must always be recomputed, never left behind: keeping the previous
        // track's clip meant note edits and Delete acted on a track that was no longer selected.
        let first = self
            .project()
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .and_then(|inner| inner.clips.first())
            .map(|clip| clip.id);
        self.select_clip(first);
        if self.selected_clip.is_some() {
            self.center_roll_on_selection();
        }
        self.reveal_track(track);
    }

    /// Selects the track a press landed on, keeping a selection that already includes what was
    /// pressed.
    ///
    /// [`Self::select_track`] narrows the clip selection to that track's first clip, which is
    /// right for a press on a header and wrong for a press on a clip that spans tracks with
    /// others — grabbing one of several selected clips must not drop the rest.
    pub(crate) fn select_track_for_press(&mut self, track: TrackId, clip: Option<ClipId>) {
        if press_keeps_selection(&self.selected_clips, clip) {
            self.selected_track = Some(track);
        } else {
            self.select_track(track);
        }
    }

    /// Re-points the selection at objects that still exist, after the document was replaced.
    fn resync_selection(&mut self) {
        self.selected_track = self
            .selected_track
            .filter(|id| self.project().track(*id).is_some())
            .or_else(|| self.project().tracks.first().map(|track| track.id));
        // Anything the new document does not contain drops out of the selection rather than
        // lingering as an id that resolves to nothing.
        let surviving = self
            .selected_clips
            .iter()
            .copied()
            .filter(|id| self.clip_exists(*id))
            .collect();
        let primary = self.selected_clip.filter(|id| self.clip_exists(*id));
        self.select_clips(surviving, primary);
        self.selected_notes.clear();
        self.drag = None;
    }

    /// Drops the note selection when `clip`'s notes have just been rewritten.
    ///
    /// The selection is plain indices into the clip's note list, and a regenerated clip has a
    /// new list — different notes, different order, different length. Kept, the old indices
    /// name whatever landed at those positions: the roll paints them selected, and Delete,
    /// Transpose or a velocity drag edits notes the user never chose. Every path that reaches
    /// `Session::set_clip_recipe` or its relatives calls this on success.
    pub(crate) fn forget_rewritten_notes(&mut self, clip: ClipId) {
        if self.selected_clip == Some(clip) {
            self.selected_notes.clear();
        }
    }

    /// Toggles a track's mute.
    pub(crate) fn toggle_mute(&mut self, track: TrackId) {
        let muted = self.project().track(track).is_some_and(|t| t.mixer.mute);
        let _ = self.session.set_track_mute(track, !muted);
    }

    /// Toggles a track's solo.
    pub(crate) fn toggle_solo(&mut self, track: TrackId) {
        let soloed = self.project().track(track).is_some_and(|t| t.mixer.solo);
        let _ = self.session.set_track_solo(track, !soloed);
    }

    /// Appends an instrument track using the first registered instrument.
    pub(crate) fn add_instrument_track(&mut self) {
        let count = self.project().tracks.len() + 1;
        let name = messages::new_track_name(self.language(), count);
        match self.session.add_default_instrument_track(name) {
            Ok(id) => {
                self.selected_track = Some(id);
                // A brand-new track has no clips, so nothing should stay selected from the old one.
                self.select_clip(None);
                self.selected_notes.clear();
                // A track added past the bottom of the panel would otherwise look like a command
                // that did nothing at all.
                self.reveal_track(id);
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdAddInstrumentTrack, &error)),
        }
    }

    /// Appends an empty audio track.
    pub(crate) fn add_audio_track(&mut self) {
        let count = self.project().tracks.len() + 1;
        let name = messages::new_audio_track_name(self.language(), count);
        let id = self.session.add_audio_track(name);
        self.selected_track = Some(id);
        self.select_clip(None);
        self.selected_notes.clear();
        self.reveal_track(id);
    }

    /// Appends a bus, which nothing is routed to yet.
    pub(crate) fn add_bus_track(&mut self) {
        let count = self.project().tracks.len() + 1;
        let name = messages::new_bus_name(self.language(), count);
        let id = self.session.add_bus_track(name);
        self.selected_track = Some(id);
        self.select_clip(None);
        self.selected_notes.clear();
        self.reveal_track(id);
    }

    /// Points a track's output at a bus, or back at the master.
    pub(crate) fn set_track_output(&mut self, track: TrackId, output: Output) {
        if let Err(error) = self.session.set_track_output(track, output) {
            self.set_failed_status(self.failure(Key::CmdSetTrackOutput, &error));
        }
    }

    /// Adds a send from a track to a bus.
    pub(crate) fn add_send(&mut self, track: TrackId, bus: TrackId) {
        if let Err(error) = self.session.add_send(track, bus) {
            self.set_failed_status(self.failure(Key::CmdAddSend, &error));
        }
    }

    /// Removes a send from a track.
    pub(crate) fn remove_send(&mut self, track: TrackId, send: SendId) {
        if let Err(error) = self.session.remove_send(track, send) {
            self.set_failed_status(self.failure(Key::CmdRemoveSend, &error));
        }
    }

    /// Moves a send's tap before or after the fader.
    pub(crate) fn toggle_send_pre_fader(&mut self, track: TrackId, send: SendId) {
        let pre_fader = self
            .project()
            .track(track)
            .and_then(|track| track.sends.iter().find(|existing| existing.id == send))
            .is_some_and(|send| send.pre_fader);
        let _ = self.session.set_send_pre_fader(track, send, !pre_fader);
    }

    /// Deletes the selected track.
    pub(crate) fn delete_selected_track(&mut self) {
        let Some(id) = self.selected_track else {
            return;
        };
        if self.session.remove_track(id).is_ok() {
            self.selected_track = self.project().tracks.first().map(|track| track.id);
            self.select_clip(None);
            self.selected_notes.clear();
        }
    }

    /// Creates an empty MIDI clip at `start` on an instrument track.
    pub(crate) fn create_clip_at(&mut self, track: TrackId, start: Ticks) {
        let length = self
            .project()
            .signatures
            .signature_at(start)
            .ticks_per_bar();
        let count = self.project().tracks.len();
        let name = messages::new_clip_name(self.language(), count);
        match self.session.add_midi_clip(track, name, start, length) {
            Ok(id) => {
                self.select_clip(Some(id));
                self.selected_notes.clear();
            }
            // Which of the two kinds refused says which sentence is true: an audio track has
            // clips and they arrive by import, and a bus has none at all.
            Err(_) => {
                let is_bus = self
                    .project()
                    .track(track)
                    .is_some_and(|track| track.kind.is_bus());
                self.set_status(self.t(match is_bus {
                    true => Key::BusHoldsNoClips,
                    false => Key::AudioClipsComeFromImport,
                }));
            }
        }
    }

    /// The time-zoom slider, for whichever view is drawing its own chrome.
    ///
    /// The arrangement and the piano roll share one [`crate::ui::timeline::TimelineView`], so
    /// both sliders drive the same value and move together. That is the honest rendering of what
    /// the application does: the two views agree pixel-for-pixel about where a tick lands, which
    /// is why a clip and its notes line up at all.
    pub(crate) fn zoom_slider(
        &self,
        id: &'static str,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        gpui::IntoElement::into_any_element(crate::ui::widgets::zoom_slider(
            id,
            self.timeline.zoom_fraction(),
            &theme,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                this.begin_drag(Drag::TimeZoom {
                    start_fraction: this.timeline.zoom_fraction(),
                    start_x: event.position.x,
                });
            }),
            cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                let notches = f32::from(event.delta.pixel_delta(gpui::px(16.0)).y) / 16.0;
                let fraction = this.timeline.zoom_fraction() + notches * 0.04;
                this.timeline.set_zoom_fraction(fraction);
                cx.notify();
            }),
        ))
    }

    /// Shifts a dragged selection by however many lanes the pointer has crossed.
    ///
    /// The whole selection moves by one delta rather than each clip landing under the pointer,
    /// so two clips a track apart are still a track apart when they are dropped. A delta that
    /// would push any of them off either end of the track list, or onto a track of the wrong
    /// kind, is refused entirely — dropping half a selection somewhere is not what the gesture
    /// meant, and the timeline move that came with it still stands.
    pub(crate) fn move_clips_by_lane(
        &mut self,
        origin_lanes: &[(ClipId, usize)],
        grab_lane: usize,
        under_pointer: TrackId,
    ) {
        let Some(target_lane) = self.project().track_index(under_pointer) else {
            return;
        };
        let delta = target_lane as isize - grab_lane as isize;
        if delta == 0 || origin_lanes.is_empty() {
            return;
        }

        let mut moves = Vec::with_capacity(origin_lanes.len());
        for (clip, lane) in origin_lanes {
            let Some(destination) = lane
                .checked_add_signed(delta)
                .and_then(|lane| self.project().tracks.get(lane))
                .map(|track| track.id)
            else {
                return;
            };
            if !self.session.clip_fits_track(*clip, destination) {
                return;
            }
            moves.push((*clip, destination));
        }
        if let Err(error) = self.session.move_clips_to_track(&moves) {
            self.set_failed_status(self.failure(Key::EditMoveClip, &error));
        }
    }

    /// Selects a clip and shows it in the editor.
    ///
    /// What a double-click on a region does in every editor that has regions, and what the clip
    /// menu's Edit does. Shared so the two cannot drift into opening it slightly differently.
    pub(crate) fn open_clip_in_editor(&mut self, clip: ClipId) {
        self.select_clip(Some(clip));
        self.selected_notes.clear();
        self.show_panel(crate::dock::Panel::PianoRoll);
        self.center_roll_on_selection();
    }

    /// Deletes whatever the current selection covers.
    pub(crate) fn delete_selection(&mut self) {
        if let Some(clip) = self.selected_clip
            && !self.selected_notes.is_empty()
        {
            let doomed: Vec<usize> = self.selected_notes.iter().copied().collect();
            let _ = self.session.remove_notes(clip, &doomed);
            self.selected_notes.clear();
            return;
        }
        if !self.selected_clips.is_empty() {
            let doomed: Vec<ClipId> = self.selected_clips.iter().copied().collect();
            if self.session.remove_clips(&doomed).is_ok() {
                self.select_clip(None);
            }
        }
    }

    /// Steps back one edit.
    pub(crate) fn undo(&mut self) {
        match self.session.undo() {
            Some(edit) => {
                self.resync_selection();
                let what = self.t(edit_key(edit));
                self.set_status(messages::undid(self.language(), what));
            }
            None => self.set_status(self.t(Key::NothingToUndo)),
        }
    }

    /// Steps forward one edit.
    pub(crate) fn redo(&mut self) {
        match self.session.redo() {
            Some(edit) => {
                self.resync_selection();
                let what = self.t(edit_key(edit));
                self.set_status(messages::redid(self.language(), what));
            }
            None => self.set_status(self.t(Key::NothingToRedo)),
        }
    }

    /// Replaces the document with an empty project, asking first if that would lose work.
    ///
    /// `Session::new_project` clears the undo history along with the document, so there is no
    /// getting the old one back afterwards — and `secondary-n` is one key away from two other
    /// bindings. The sheet finishes the command itself once it has an answer.
    pub(crate) fn new_project_asking(&mut self) -> bool {
        if !self.confirm_discard(crate::ui::prompt::PendingAction::NewProject) {
            return false;
        }
        self.new_project();
        true
    }

    /// Replaces the document with an empty project.
    pub(crate) fn new_project(&mut self) {
        self.session.new_project();
        self.resync_selection();
        self.reset_view();
        self.set_status(self.t(Key::NewProjectStatus));
    }

    /// Saves to the current path, prompting when there is not one yet.
    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.path().is_none() {
            self.save_as(window, cx);
            return;
        }
        match self.session.save_in_place() {
            Ok(()) => {
                let path = self.session.path().map(|p| p.display().to_string());
                self.set_status(messages::saved(self.language(), &path.unwrap_or_default()));
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
        }
    }

    /// Prompts for a path and saves there.
    ///
    /// What lands on disk is a project *folder*: choosing `MySong.auris` writes
    /// `MySong/MySong.auris` with the song's audio beside it, so the whole thing can be moved or
    /// handed to someone else afterwards. The status line names the document that resulted rather
    /// than the path that was typed, since they differ.
    pub(crate) fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_then(None, window, cx);
    }

    /// [`Self::save_as`], carrying on with `then` once the file has landed.
    ///
    /// The follow-up has to travel with the dialog rather than be done when this returns: the
    /// picker is asynchronous, so "save, then close the window" would otherwise close it while
    /// the user was still choosing a name.
    pub(crate) fn save_as_then(
        &mut self,
        then: Option<crate::ui::prompt::PendingAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.project().name.clone();
        let language = self.language();
        // Through the window rather than the app, so the continuation gets a `Window` back: the
        // follow-up may be "and now close it", which there is no other way to do from here.
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let handle = rfd::AsyncFileDialog::new()
                    .set_title(Key::DialogSaveProject.get(language))
                    .set_file_name(format!("{name}.{}", auris_session::PROJECT_EXTENSION))
                    .add_filter(
                        Key::FilterProject.get(language),
                        &[auris_session::PROJECT_EXTENSION],
                    )
                    .save_file()
                    .await;
                let Some(handle) = handle else { return };
                let path = handle.path().to_path_buf();
                let _ = view.update_in(cx, |this, window, cx| {
                    this.finish_save_as(&path, then, window, cx);
                    cx.notify();
                });
            })
            .detach();
    }

    /// Writes the document at the chosen path, or asks before replacing what is there.
    fn finish_save_as(
        &mut self,
        path: &std::path::Path,
        then: Option<crate::ui::prompt::PendingAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.session.save_as(path) {
            Ok(report) => {
                self.report_save(&report);
                if let Some(next) = then {
                    self.run_pending(next, window, cx);
                }
            }
            // The system dialog checked for a collision at the name that was typed. A project is
            // written one folder deeper than that, so it never saw this one.
            Err(SessionError::WouldReplace(existing)) => {
                self.open_prompt(crate::ui::prompt::Prompt::ask(
                    self.t(Key::ReplaceTitle),
                    crate::ui::prompt::Question::Replace {
                        chosen: path.to_path_buf(),
                        existing,
                        then,
                    },
                ));
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
        }
    }

    /// Reports where a project landed, and what did not travel with it.
    pub(crate) fn report_save(&mut self, report: &auris_session::SaveReport) {
        let language = self.language();
        let shown = report.document.display().to_string();
        self.set_status(if report.uncollected.is_empty() {
            messages::saved(language, &shown)
        } else {
            // The folder is not portable, and the only way to find that out used to be opening
            // it on another machine and hearing silence.
            messages::saved_uncollected(language, &shown, report.uncollected.len())
        });
    }

    /// Copies everything the project refers to into its folder.
    ///
    /// A SoundFont library runs to hundreds of megabytes, so this says what it is doing and
    /// gives the window a frame to say it in before starting.
    pub(crate) fn collect_assets(&mut self, cx: &mut Context<Self>) {
        self.set_status(messages::collecting(self.language()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;
            let _ = this.update(cx, |this, cx| {
                let language = this.language();
                match this.session.collect_assets() {
                    Ok(0) => this.set_status(messages::assets_already_collected(language)),
                    Ok(count) => this.set_status(messages::assets_collected(language, count)),
                    Err(error) => {
                        this.set_failed_status(this.failure(Key::CmdCollectAssets, &error))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts the view back to the top left, for a document that is not the one it was showing.
    ///
    /// Opening a project while scrolled out to bar 400 showed an empty timeline, and there was
    /// no command for going back — the scroll is not part of the document, so nothing restored
    /// it either.
    pub(crate) fn reset_view(&mut self) {
        self.timeline.scroll_ticks = Ticks::ZERO;
        self.lane_scroll = gpui::px(0.0);
    }

    /// Prompts for a project file and opens it, asking first if that would lose work.
    ///
    /// Opening an old project "just to look at something" is routine, and `Session::open` clears
    /// the undo history, so the document on screen has to be dealt with before the picker opens
    /// rather than after a file has been chosen.
    pub(crate) fn open_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_discard(crate::ui::prompt::PendingAction::OpenProject) {
            self.pick_and_open_project(cx);
        }
    }

    /// Prompts for a project file and opens it, with the document already dealt with.
    pub(crate) fn pick_and_open_project(&mut self, cx: &mut Context<Self>) {
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogOpenProject.get(language))
                .add_filter(
                    Key::FilterProject.get(language),
                    &[auris_session::PROJECT_EXTENSION],
                )
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let _ = this.update(cx, |this, cx| {
                this.open_project_at(handle.path().to_path_buf(), cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the project at `path`, with the document already dealt with.
    ///
    /// The end of both ways in: the file dialog picks a path and lands here, and a dropped
    /// project arrives here already knowing one.
    pub(crate) fn open_project_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                let text = messages::opening(this.language(), &path.display().to_string());
                this.set_status(text);
                cx.notify();
            });
            // A project decodes every audio file it names, which on a real song is seconds of
            // work on this thread. Without a painted frame first the window simply freezes.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            let _ = this.update(cx, |this, cx| {
                match this.session.open(&path) {
                    Ok(missing) => {
                        this.resync_selection();
                        // A different document, so the view of the old one means nothing.
                        this.reset_view();
                        let language = this.language();
                        let shown = path.display().to_string();
                        this.set_status(match missing.len() {
                            0 => messages::opened(language, &shown),
                            1 => messages::opened_missing_one(
                                language,
                                &shown,
                                &missing[0].display().to_string(),
                            ),
                            n => messages::opened_missing_many(language, &shown, n),
                        });
                        // Which files, not how many. The clips that lost their audio are
                        // indistinguishable from silence, and a count in a status line that the
                        // next command overwrites left the log as the only way to find out.
                        if missing.len() > 1 {
                            this.open_prompt(crate::ui::prompt::Prompt::notice(
                                this.t(Key::MissingAudioTitle),
                                missing.iter().map(|path| path.display().to_string().into()),
                            ));
                        }
                    }
                    Err(error) => this.set_failed_status(this.failure(Key::CmdOpenProject, &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Prompts for a MIDI file and reads it as a new document, asking first if that would lose
    /// work.
    ///
    /// Guarded before the picker rather than after a file has been chosen, the same way opening a
    /// project is: the document on screen has to be dealt with before a dialog goes up, or the
    /// user answers a question about work they have already stopped thinking about.
    pub(crate) fn import_midi(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_discard(crate::ui::prompt::PendingAction::ImportMidiPicked) {
            return;
        }
        self.pick_and_import_midi(cx);
    }

    /// Prompts for a MIDI file, with the document already dealt with.
    pub(crate) fn pick_and_import_midi(&mut self, cx: &mut Context<Self>) {
        let extensions: Vec<String> = auris_session::midi_extensions()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogImportMidi.get(language))
                .add_filter(Key::FilterMidi.get(language), &extension_refs)
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let _ = this.update(cx, |this, cx| {
                this.import_midi_at(handle.path().to_path_buf(), cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the MIDI file at `path` as a new document, with the old one already dealt with.
    ///
    /// The end of both ways in: the file dialog picks a path and lands here, and a dropped `.mid`
    /// arrives here already knowing one.
    pub(crate) fn import_midi_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let shown = this.update(cx, |this, cx| {
                let text = messages::opening(this.language(), &path.display().to_string());
                this.set_status(text);
                cx.notify();
            });
            if shown.is_err() {
                return;
            }
            // A large MIDI file is tens of thousands of events, and the tracks it makes are built
            // on this thread — so let the status line paint before any of that starts.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            let _ = this.update(cx, |this, cx| {
                match this.session.import_midi(&path) {
                    Ok(report) => {
                        this.resync_selection();
                        this.reset_view();
                        let first = this.project().tracks.first().map(|track| track.id);
                        this.selected_track = None;
                        if let Some(track) = first {
                            this.select_track(track);
                        }
                        let language = this.language();
                        this.set_status(messages::midi_imported(
                            language,
                            report.tracks,
                            report.notes,
                        ));
                    }
                    Err(error) => {
                        this.set_failed_status(this.failure(Key::CmdImportMidi, &error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Prompts for a destination and writes the document out as a MIDI file.
    pub(crate) fn export_midi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.project().name.clone();
        let language = self.language();
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let handle = rfd::AsyncFileDialog::new()
                    .set_title(Key::DialogExportMidi.get(language))
                    .set_file_name(format!("{name}.mid"))
                    .add_filter(Key::FilterMidi.get(language), &["mid"])
                    .save_file()
                    .await;
                let Some(handle) = handle else { return };
                let path = handle.path().to_path_buf();
                let _ = view.update(cx, |this, cx| {
                    match this.session.export_midi(&path) {
                        Ok(notes) => {
                            let language = this.language();
                            this.set_status(messages::midi_exported(
                                language,
                                &path.display().to_string(),
                                notes,
                            ));
                        }
                        Err(error) => {
                            this.set_failed_status(this.failure(Key::CmdExportMidi, &error));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
    }

    /// Prompts for a song specification and replaces the document with what it describes.
    ///
    /// The whole piece is one undo step, so a composition that is not what was wanted is one
    /// press away from the document that was there before it.
    pub(crate) fn compose_from_spec(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogComposeSpec.get(language))
                .add_filter(
                    Key::FilterSpec.get(language),
                    &[auris_session::SPEC_EXTENSION],
                )
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();

            let _ = this.update(cx, |this, cx| {
                let text = messages::composing(this.language(), &path.display().to_string());
                this.set_status(text);
                cx.notify();
            });
            // Writing a whole piece is the slowest thing here that is not a render.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            let _ = this.update(cx, |this, cx| {
                this.compose_file(&path);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads, parses and composes one specification file.
    ///
    /// Split out of the dialog so the failure paths are reachable without one: a file that will
    /// not open, will not parse, or asks for nothing this build can play.
    fn compose_file(&mut self, path: &std::path::Path) {
        let language = self.language();
        let shown = path.display().to_string();

        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.set_failed_status(messages::failed(
                    language,
                    self.t(Key::CmdComposeSong),
                    &error.to_string(),
                ));
                return;
            }
        };
        // The parser reports every complaint it has, not just the first, so a specification with
        // three typos takes one round trip rather than three.
        let spec = match SongSpec::parse(&text) {
            Ok(spec) => spec,
            Err(errors) => {
                // Every complaint, in a sheet. They were joined with newlines into the status
                // bar, which is one row twenty-two pixels tall: the parser's whole point — say
                // all of it at once — was thrown away by where the answer was put.
                self.set_failed_status(messages::spec_rejected(language, &shown));
                self.open_prompt(crate::ui::prompt::Prompt::notice(
                    self.t(Key::SpecRejectedTitle),
                    errors.iter().map(|error| error.to_string().into()),
                ));
                return;
            }
        };

        let piece = compose(&spec);
        let seed = piece.seed;
        match self.session.compose(&piece) {
            Ok(report) => {
                self.resync_selection();
                self.reset_view();
                // Point the editors at the first part rather than leaving them empty. Opening a
                // project does not need this because its own selection is restored; a freshly
                // composed document has no selection to restore, and an empty piano roll over a
                // piece full of notes reads as a failure.
                let first = self.project().tracks.first().map(|track| track.id);
                self.selected_track = None;
                if let Some(track) = first {
                    self.select_track(track);
                }
                self.set_status(if report.substituted.is_empty() {
                    messages::composed_document(language, report.tracks, report.notes, seed)
                } else {
                    messages::composed_document_substituted(
                        language,
                        report.tracks,
                        report.notes,
                        seed,
                        report.substituted.len(),
                    )
                });
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdComposeSong, &error)),
        }
    }

    /// Prompts for an audio file and drops it onto a new audio track.
    pub(crate) fn import_audio(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let extensions: Vec<String> = auris_session::supported_audio_extensions()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogImportAudio.get(language))
                .add_filter(Key::FilterAudio.get(language), &extension_refs)
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();

            let _ = this.update(cx, |this, cx| {
                let text = messages::importing(this.language(), &path.display().to_string());
                this.set_status(text);
                cx.notify();
            });

            // Decoding is slow enough to drop frames, but it has to land back in the session,
            // which lives on this thread — so yield first and let the status paint.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            let _ = this.update(cx, |this, cx| {
                let start = this.playhead_ticks();
                match this.take_audio(&path, start) {
                    Ok(line) => this.set_status(line),
                    Err(error) => this.set_failed_status(this.failure(Key::CmdImportAudio, &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads one audio file onto a new track, and says so.
    ///
    /// The file dialog and a drop both land here, which is what stops the two from drifting apart
    /// — a gesture that imported a file without selecting the track it made would be a gesture
    /// whose result the user has to go and find.
    fn take_audio(&mut self, path: &Path, start: Ticks) -> Result<String, SessionError> {
        self.session.import_audio(path, start)?;
        // The track it made is the last one, and it is scrolled to rather than merely selected:
        // an import onto an arrangement taller than the window otherwise lands out of sight.
        if let Some(track) = self.project().tracks.last().map(|track| track.id) {
            self.select_track(track);
        }
        Ok(messages::imported(
            self.language(),
            &path.display().to_string(),
        ))
    }

    /// Prompts for a SoundFont and adds it to the project's library.
    ///
    /// Unlike importing audio this puts nothing on the timeline: a font is a shelf of sounds, and
    /// which track plays which one is a separate choice made in the library.
    pub(crate) fn import_soundfont(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let extensions: Vec<String> = auris_session::supported_soundfont_extensions()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogImportSoundFont.get(language))
                .add_filter(Key::FilterSoundFont.get(language), &extension_refs)
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();

            let _ = this.update(cx, |this, cx| {
                let text = messages::importing(this.language(), &path.display().to_string());
                this.set_status(text);
                cx.notify();
            });

            // A font can be hundreds of megabytes, so the read is far too slow to do without
            // having painted the status line first.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            let _ = this.update(cx, |this, cx| {
                match this.take_soundfont(&path) {
                    Ok(line) => this.set_status(line),
                    Err(error) => {
                        this.set_failed_status(this.failure(Key::CmdImportSoundFont, &error))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads one SoundFont into the library, opens it there, and says what it holds.
    fn take_soundfont(&mut self, path: &Path) -> Result<String, SessionError> {
        let id = self.session.import_soundfont(path)?;
        let name = self
            .session
            .soundfonts()
            .find(|font| font.id == id)
            .map(|font| font.name.clone())
            .unwrap_or_default();
        let sounds = self.session.soundfont_presets(id).len();
        // Show what just arrived. The library is the only place these sounds can be chosen from,
        // and importing a font is the act of going to choose one.
        self.show_panel(crate::dock::Panel::Library);
        self.library
            .set_open(crate::ui::library::Branch::SoundFonts, true);
        self.library
            .set_open(crate::ui::library::Branch::Font(id), true);
        Ok(messages::soundfont_imported(self.language(), &name, sounds))
    }

    /// Does what a drop is asking for: opens a project, or reads files into the open one.
    ///
    /// The one destructive answer goes through the same guard the Open command does, so a drop
    /// that would throw away unsaved work asks first and can be answered with Save.
    pub(crate) fn accept_drop(
        &mut self,
        paths: Vec<PathBuf>,
        start: Ticks,
        cx: &mut Context<Self>,
    ) {
        match drop_action(&paths) {
            DropAction::Open(path) => {
                if self.confirm_discard(crate::ui::prompt::PendingAction::OpenDropped(path.clone()))
                {
                    self.open_project_at(path, cx);
                }
            }
            DropAction::OpenMidi(path) => {
                if self.confirm_discard(crate::ui::prompt::PendingAction::ImportMidi(path.clone()))
                {
                    self.import_midi_at(path, cx);
                }
            }
            DropAction::Import(dropped) => self.import_dropped(dropped, start, cx),
            DropAction::Confused => {
                self.set_failed_status(messages::project_wants_to_be_alone(self.language()))
            }
        }
        cx.notify();
    }

    /// Imports dropped files, in the order they were dropped.
    ///
    /// `start` is where dropped audio lands on the timeline. A font ignores it: a font goes on a
    /// shelf rather than onto a track, and where on the timeline it was let go means nothing.
    ///
    /// One file at a time, on this thread, with the status line repainted between each. Decoding
    /// is slow enough that a drop of a folder of takes would otherwise be several seconds of a
    /// frozen window with nothing to say what was happening.
    fn import_dropped(&mut self, dropped: Dropped, start: Ticks, cx: &mut Context<Self>) {
        let language = self.language();
        let mut outcome = DropOutcome::default();
        for path in &dropped.rejected {
            outcome.failed.push(messages::cannot_import(
                language,
                &path.display().to_string(),
            ));
        }
        if dropped.accepted.is_empty() {
            // Nothing to read, so nothing to wait for: say so now rather than a frame later.
            self.report(outcome.summary(language));
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            for (path, kind) in dropped.accepted {
                let shown = this.update(cx, |this, cx| {
                    let text = messages::importing(this.language(), &path.display().to_string());
                    this.set_status(text);
                    cx.notify();
                });
                if shown.is_err() {
                    return;
                }

                // A font can be hundreds of megabytes and an audio file is decoded and resampled,
                // so both are far too slow to start without having painted that line first.
                cx.background_executor()
                    .timer(std::time::Duration::ZERO)
                    .await;

                let done = this.update(cx, |this, cx| {
                    let done = match kind {
                        DropKind::Audio => this.take_audio(&path, start),
                        DropKind::SoundFont => this.take_soundfont(&path),
                        // `drop_action` sent everything that replaces the document down the other
                        // branch, and one that reached here would be a document quietly not
                        // opened.
                        DropKind::Project | DropKind::Midi => {
                            unreachable!("a whole document is opened, not imported into one")
                        }
                    };
                    cx.notify();
                    // Turned into a line here, while the view that knows the language is in hand.
                    done.map_err(|error| this.failure(import_key(kind), &error))
                });
                match done {
                    Ok(Ok(line)) => outcome.imported.push(line),
                    Ok(Err(line)) => outcome.failed.push(line),
                    // The window has gone; there is nobody left to report to.
                    Err(_) => return,
                }
            }

            let _ = this.update(cx, |this, cx| {
                let language = this.language();
                this.report(outcome.summary(language));
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows a track's automation lane on `target`, or hides it when it is already on that one.
    ///
    /// One lane per track: choosing a different parameter swaps what the row draws rather than
    /// stacking a second row. Clicking the parameter that is already showing closes it, which is
    /// what makes the same menu item both the way in and the way out.
    pub(crate) fn show_automation(&mut self, track: TrackId, target: ParamTarget) {
        match self.automation_lanes.get(&track) {
            Some(showing) if *showing == target => {
                self.automation_lanes.remove(&track);
            }
            _ => {
                self.automation_lanes.insert(track, target);
                // The row appears under the track, which on a full arrangement is past the bottom
                // of the panel — a command that opened something out of sight reads as one that
                // did nothing.
                self.reveal_track(track);
            }
        }
    }

    /// Puts a summary on the status line, in the colour its outcome deserves.
    fn report(&mut self, summary: Option<(String, bool)>) {
        let Some((line, succeeded)) = summary else {
            return;
        };
        match succeeded {
            true => self.set_status(line),
            false => self.set_failed_status(line),
        }
    }

    /// Prompts for a destination and renders the project to a WAV file.
    pub(crate) fn start_export(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_export(false, cx);
    }

    /// Prompts for a destination and renders only the cycle region.
    pub(crate) fn start_export_cycle(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_export(true, cx);
    }

    /// The export flow behind both commands: the whole arrangement, or the cycle region.
    fn begin_export(&mut self, cycle: bool, cx: &mut Context<Self>) {
        // `export` is not set until a path comes back, so the running check alone let a second
        // Export through while the picker was still up — two renders, and the summary of
        // whichever finished second.
        if self.choosing_export || self.export.as_ref().is_some_and(|e| e.result.is_none()) {
            self.set_status(self.t(Key::ExportAlreadyRunning));
            return;
        }
        // A snapshot, so the render is unaffected by anything edited while it runs.
        let job = self.session.render_job();
        let options = if cycle {
            // Refused before the dialog opens: a save sheet for a region that does not exist
            // would collect a filename for nothing.
            match job.loop_options(OfflineOptions::whole_project()) {
                Some(options) => options,
                None => {
                    self.set_failed_status(self.t(Key::NoCycleToExport));
                    return;
                }
            }
        } else {
            OfflineOptions::whole_project()
        };
        // Which command failed, when one does — and a different suggested name, so a cycle
        // bounced next to a full export does not offer to overwrite it.
        let command = if cycle {
            Key::CmdExportCycle
        } else {
            Key::CmdExportWav
        };
        self.choosing_export = true;
        let name = self.project().name.clone();
        let suggested = if cycle {
            format!("{name} (cycle).wav")
        } else {
            format!("{name}.wav")
        };
        let language = self.language();

        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogExportWav.get(language))
                .set_file_name(suggested)
                .add_filter(Key::FilterWav.get(language), &["wav"])
                .save_file()
                .await;
            let Some(handle) = handle else {
                let _ = this.update(cx, |this, _| this.choosing_export = false);
                return;
            };
            let path = handle.path().to_path_buf();

            let progress = Arc::new(AtomicU32::new(0));
            let _ = this.update(cx, |this, cx| {
                this.choosing_export = false;
                this.export = Some(ExportState {
                    path: path.clone(),
                    progress: Arc::clone(&progress),
                    result: None,
                });
                cx.notify();
            });

            let render_path = path.clone();
            let rendered = cx
                .background_executor()
                .spawn(async move {
                    job.render_to_wav(
                        &render_path,
                        &WavExportSettings::default(),
                        &options,
                        &mut |fraction| progress.store(fraction.to_bits(), Ordering::Relaxed),
                    )
                    .map_err(|error| error.to_string())
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let language = this.language();
                let message = match rendered {
                    Ok(summary) => {
                        let text = messages::exported(
                            language,
                            &path.display().to_string(),
                            &Seconds(summary.seconds).format_clock(),
                            summary.peak_db,
                        );
                        this.set_status(text.clone());
                        Ok(text)
                    }
                    Err(error) => {
                        let text = messages::failed(language, command.get(language), &error);
                        // The failure colour, like every other Err arm: once the overlay is
                        // dismissed the status line is the only record of the failure, and in
                        // the ordinary grey it read as just another note.
                        this.set_failed_status(text.clone());
                        Err(text)
                    }
                };
                if let Some(export) = this.export.as_mut() {
                    export.result = Some(message);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scrolls the piano roll to the middle of the selected clip's pitch range.
    ///
    /// Without this, selecting a bass part while the roll is parked two octaves up shows an
    /// empty grid and looks like the clip is missing.
    pub(crate) fn center_roll_on_selection(&mut self) {
        let Some(range) = self
            .selected_midi_clip()
            .and_then(|clip| clip.pitch_range())
        else {
            return;
        };
        let middle = ((range.0 as u16 + range.1 as u16) / 2) as u8;
        let body_height =
            crate::theme::Metrics::BOTTOM_DOCK_HEIGHT - crate::theme::Metrics::PANEL_HEADER_HEIGHT;
        self.pitch.center_on(middle, body_height);
    }

    /// `"Could not <action>: <reason>"`, with both halves translated.
    pub(crate) fn failure(&self, action: Key, error: &SessionError) -> String {
        messages::failed(
            self.language(),
            self.t(action),
            &error_text(error, self.language()),
        )
    }

    /// Length of an audio clip on the musical timeline.
    pub(crate) fn audio_clip_length_ticks(&self, clip: &AudioClip) -> Ticks {
        self.session.audio_clip_length_ticks(clip)
    }
}

/// The command a dropped file would have been, so a failure names the same thing the menu does.
fn import_key(kind: DropKind) -> Key {
    match kind {
        DropKind::Audio => Key::CmdImportAudio,
        DropKind::SoundFont => Key::CmdImportSoundFont,
        DropKind::Project => Key::CmdOpenProject,
        DropKind::Midi => Key::CmdImportMidi,
    }
}

/// Whether a press on `clip` should leave the clip selection as it is.
///
/// Pulled out of the view so the rule can be tested: pressing a clip that is already part of a
/// selection spanning several tracks used to narrow the selection down to that track's first
/// clip, so dragging one of several selected clips moved only that one.
fn press_keeps_selection(selected: &BTreeSet<ClipId>, clip: Option<ClipId>) -> bool {
    clip.is_some_and(|id| selected.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressing_a_clip_that_is_already_selected_keeps_the_rest() {
        let selected = BTreeSet::from([ClipId(1), ClipId(2)]);
        assert!(press_keeps_selection(&selected, Some(ClipId(2))));
    }

    #[test]
    fn pressing_anywhere_else_starts_a_new_selection() {
        let selected = BTreeSet::from([ClipId(1), ClipId(2)]);
        assert!(!press_keeps_selection(&selected, Some(ClipId(3))));
        assert!(
            !press_keeps_selection(&selected, None),
            "a press on empty lane space is not a press on a selected clip"
        );
        assert!(!press_keeps_selection(&BTreeSet::new(), Some(ClipId(1))));
    }
}
