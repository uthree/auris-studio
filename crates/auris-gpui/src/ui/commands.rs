//! What the menus, buttons and key bindings do.
//!
//! These are thin: the document work happens in `auris-session`, and what remains here is the
//! desktop half — file dialogs, keeping the selection pointing at something that still exists,
//! moving long work off the main thread, and turning results into a status line.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use std::collections::BTreeSet;

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::{Context, Window};

use crate::app::{AurisApp, Drag, ExportState};
use crate::i18n::{edit_key, error_text};

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
            }
            Err(error) => self.set_status(self.failure(Key::CmdAddInstrumentTrack, &error)),
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
        let length = self.project().time_signature.ticks_per_bar();
        let count = self.project().tracks.len();
        let name = messages::new_clip_name(self.language(), count);
        match self.session.add_midi_clip(track, name, start, length) {
            Ok(id) => {
                self.select_clip(Some(id));
                self.selected_notes.clear();
            }
            Err(_) => {
                self.set_status(self.t(Key::AudioClipsComeFromImport));
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
            self.set_status(self.failure(Key::EditMoveClip, &error));
        }
    }

    /// Selects a clip and shows it in the editor.
    ///
    /// What a double-click on a region does in every editor that has regions, and what the clip
    /// menu's Edit does. Shared so the two cannot drift into opening it slightly differently.
    pub(crate) fn open_clip_in_editor(&mut self, clip: ClipId) {
        self.select_clip(Some(clip));
        self.selected_notes.clear();
        self.editor = crate::app::EditorTab::PianoRoll;
        self.panels.editor_visible = true;
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

    /// Replaces the document with an empty project.
    pub(crate) fn new_project(&mut self) {
        self.session.new_project();
        self.resync_selection();
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
            Err(error) => self.set_status(self.failure(Key::CmdSave, &error)),
        }
    }

    /// Prompts for a path and saves there.
    ///
    /// What lands on disk is a project *folder*: choosing `MySong.auris` writes
    /// `MySong/MySong.auris` with the song's audio beside it, so the whole thing can be moved or
    /// handed to someone else afterwards. The status line names the document that resulted rather
    /// than the path that was typed, since they differ.
    pub(crate) fn save_as(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.project().name.clone();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Save project")
                .set_file_name(format!("{name}.{}", auris_session::PROJECT_EXTENSION))
                .add_filter("Auris project", &[auris_session::PROJECT_EXTENSION])
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                match this.session.save_as(&path) {
                    Ok(document) => {
                        let text =
                            messages::saved(this.language(), &document.display().to_string());
                        this.set_status(text);
                    }
                    Err(error) => this.set_status(this.failure(Key::CmdSave, &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies everything the project refers to into its folder.
    pub(crate) fn collect_assets(&mut self) {
        let language = self.language();
        match self.session.collect_assets() {
            Ok(0) => self.set_status(messages::assets_already_collected(language)),
            Ok(count) => self.set_status(messages::assets_collected(language, count)),
            Err(error) => self.set_status(self.failure(Key::CmdCollectAssets, &error)),
        }
    }

    /// Prompts for a project file and opens it.
    pub(crate) fn open_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Open project")
                .add_filter("Auris project", &[auris_session::PROJECT_EXTENSION])
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                match this.session.open(&path) {
                    Ok(missing) => {
                        this.resync_selection();
                        // Fold the failures into the one status line rather than setting a
                        // message that "Opened ..." would immediately overwrite.
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
                    }
                    Err(error) => this.set_status(this.failure(Key::CmdOpenProject, &error)),
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
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Compose from specification")
                .add_filter("Song specification", &[auris_session::SPEC_EXTENSION])
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();

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
                self.set_status(messages::failed(
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
                let mut message = messages::spec_rejected(language, &shown);
                for error in errors {
                    message.push_str(&format!("\n  {error}"));
                }
                self.set_status(message);
                return;
            }
        };

        let piece = compose(&spec);
        let seed = piece.seed;
        match self.session.compose(&piece) {
            Ok(report) => {
                self.resync_selection();
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
            Err(error) => self.set_status(self.failure(Key::CmdComposeSong, &error)),
        }
    }

    /// Prompts for an audio file and drops it onto a new audio track.
    pub(crate) fn import_audio(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let extensions: Vec<String> = auris_session::supported_audio_extensions()
            .iter()
            .map(|s| s.to_string())
            .collect();
        cx.spawn(async move |this, cx| {
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Import audio")
                .add_filter("Audio", &extension_refs)
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
                match this.session.import_audio(&path, start) {
                    Ok(_) => {
                        this.selected_track = this.project().tracks.last().map(|t| t.id);
                        this.select_clip(None);
                        let text = messages::imported(this.language(), &path.display().to_string());
                        this.set_status(text);
                    }
                    Err(error) => this.set_status(this.failure(Key::CmdImportAudio, &error)),
                }
                cx.notify();
            });
        })
        .detach();
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
        cx.spawn(async move |this, cx| {
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Import SoundFont")
                .add_filter("SoundFont", &extension_refs)
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
                match this.session.import_soundfont(&path) {
                    Ok(id) => {
                        let name = this
                            .session
                            .soundfonts()
                            .find(|font| font.id == id)
                            .map(|font| font.name.clone())
                            .unwrap_or_default();
                        let sounds = this.session.soundfont_presets(id).len();
                        // Show what just arrived. The library is the only place these sounds can
                        // be chosen from, and importing a font is the act of going to choose one.
                        this.panels.library_visible = true;
                        this.library
                            .set_open(crate::ui::library::Branch::SoundFonts, true);
                        this.library
                            .set_open(crate::ui::library::Branch::Font(id), true);
                        let language = this.language();
                        this.set_status(messages::soundfont_imported(language, &name, sounds));
                    }
                    Err(error) => this.set_status(this.failure(Key::CmdImportSoundFont, &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Prompts for a destination and renders the project to a WAV file.
    pub(crate) fn start_export(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.export.as_ref().is_some_and(|e| e.result.is_none()) {
            self.set_status(self.t(Key::ExportAlreadyRunning));
            return;
        }
        let name = self.project().name.clone();
        // A snapshot, so the render is unaffected by anything edited while it runs.
        let job = self.session.render_job();

        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Export WAV")
                .set_file_name(format!("{name}.wav"))
                .add_filter("WAV audio", &["wav"])
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();

            let progress = Arc::new(AtomicU32::new(0));
            let _ = this.update(cx, |this, cx| {
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
                        &OfflineOptions::whole_project(),
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
                        let text =
                            messages::failed(language, Key::CmdExportWav.get(language), &error);
                        this.set_status(text.clone());
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
            crate::theme::Metrics::EDITOR_HEIGHT - crate::theme::Metrics::EDITOR_HEADER_HEIGHT;
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
