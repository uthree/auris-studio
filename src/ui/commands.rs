//! Document commands: track and clip editing, file dialogs, import and export.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use auris_core::time::{Seconds, Ticks};
use auris_core::{Project, TrackId, TrackKind};
use auris_engine::{EngineCommand, OfflineOptions, render_project_with_progress};
use auris_io::{
    PROJECT_EXTENSION, WavExportSettings, import_audio_file, load_project, save_project,
    supported_extensions, write_wav,
};
use gpui::{Context, Window};

use crate::app::{AurisApp, ExportState};

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
        self.selected_clip = self
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .and_then(|inner| inner.clips.first())
            .map(|clip| clip.id);
        if self.selected_clip.is_some() {
            self.center_roll_on_selection();
        }
    }

    /// Scrolls the piano roll to the middle of the selected clip's pitch range.
    ///
    /// Without this, selecting a bass part while the roll is parked two octaves up shows an
    /// empty grid and looks like the clip is missing.
    pub(crate) fn center_roll_on_selection(&mut self) {
        let Some(range) = self
            .selected_midi_clip()
            .and_then(|(_, clip)| clip.pitch_range())
        else {
            return;
        };
        let middle = ((range.0 as u16 + range.1 as u16) / 2) as u8;
        let body_height =
            crate::theme::Metrics::EDITOR_HEIGHT - crate::theme::Metrics::EDITOR_HEADER_HEIGHT;
        self.pitch.center_on(middle, body_height);
    }

    /// Toggles a track's mute and tells the engine.
    pub(crate) fn toggle_mute(&mut self, track: TrackId) {
        self.edit("Mute track");
        let Some(index) = self.project.track_index(track) else {
            return;
        };
        let mute = !self.project.tracks[index].mixer.mute;
        self.project.tracks[index].mixer.mute = mute;
        self.send(EngineCommand::SetTrackMute { index, mute });
    }

    /// Toggles a track's solo.
    ///
    /// Solo changes which *other* tracks are audible, so it cannot be expressed as one
    /// per-track command; the graph is rebuilt instead.
    pub(crate) fn toggle_solo(&mut self, track: TrackId) {
        self.edit("Solo track");
        if let Some(track) = self.project.track_mut(track) {
            track.mixer.solo = !track.mixer.solo;
        }
        self.rebuild_graph();
    }

    /// Appends an instrument track using the first registered instrument.
    pub(crate) fn add_instrument_track(&mut self) {
        let Some(instrument) = self.registry.first_instrument_id().map(str::to_string) else {
            self.set_status("No instruments are registered");
            return;
        };
        self.edit("Add instrument track");
        let count = self.project.tracks.len() + 1;
        let id = self
            .project
            .add_instrument_track(format!("Track {count}"), instrument);
        self.selected_track = Some(id);
        // A brand-new track has no clips, so nothing should stay selected from the old one.
        self.selected_clip = None;
        self.selected_notes.clear();
        self.rebuild_graph();
    }

    /// Appends an empty audio track.
    pub(crate) fn add_audio_track(&mut self) {
        self.edit("Add audio track");
        let count = self.project.tracks.len() + 1;
        let id = self.project.add_audio_track(format!("Audio {count}"));
        self.selected_track = Some(id);
        self.selected_clip = None;
        self.selected_notes.clear();
        self.rebuild_graph();
    }

    /// Deletes the selected track.
    pub(crate) fn delete_selected_track(&mut self) {
        let Some(id) = self.selected_track else {
            return;
        };
        self.edit("Delete track");
        self.project.remove_track(id);
        self.selected_track = self.project.tracks.first().map(|track| track.id);
        self.selected_clip = None;
        self.selected_notes.clear();
        self.rebuild_graph();
    }

    /// Creates an empty MIDI clip at `start` on an instrument track.
    pub(crate) fn create_clip_at(&mut self, track: TrackId, start: Ticks) {
        let is_instrument = self
            .project
            .track(track)
            .is_some_and(|t| matches!(t.kind, TrackKind::Instrument(_)));
        if !is_instrument {
            self.set_status("Audio clips come from Import Audio, not from an empty lane");
            return;
        }
        self.edit("Add clip");
        let length = self.project.time_signature.ticks_per_bar();
        let name = format!("Clip {}", self.project.tracks.len());
        if let Some(id) = self.project.add_midi_clip(track, name, start, length) {
            self.selected_clip = Some(id);
            self.selected_notes.clear();
        }
        self.rebuild_graph();
    }

    /// Deletes whatever the current selection covers.
    pub(crate) fn delete_selection(&mut self) {
        if !self.selected_notes.is_empty() {
            self.delete_selected_notes();
            return;
        }
        if let Some(clip) = self.selected_clip {
            self.edit("Delete clip");
            self.project.remove_clip(clip);
            self.selected_clip = None;
            self.rebuild_graph();
        }
    }

    /// Steps back one edit.
    pub(crate) fn undo(&mut self) {
        if !self.history.can_undo() {
            self.set_status("Nothing to undo");
            return;
        }
        if let Some(project) = self.history.undo(&self.project) {
            let label = self.history.redo_label().unwrap_or("edit").to_string();
            self.set_project(project);
            self.set_status(format!("Undid {label}"));
            self.dirty = true;
        }
    }

    /// Steps forward one edit.
    pub(crate) fn redo(&mut self) {
        if !self.history.can_redo() {
            self.set_status("Nothing to redo");
            return;
        }
        if let Some(project) = self.history.redo(&self.project) {
            let label = self.history.undo_label().unwrap_or("edit").to_string();
            self.set_project(project);
            self.set_status(format!("Redid {label}"));
            self.dirty = true;
        }
    }

    /// Replaces the document with an empty project.
    pub(crate) fn new_project(&mut self) {
        let mut project = Project::new("Untitled", self.project.sample_rate);
        if let Some(instrument) = self.registry.first_instrument_id() {
            project.add_instrument_track("Track 1", instrument);
        }
        self.history.clear();
        self.project_path = None;
        self.dirty = false;
        // History is cleared above, so nothing can bring the old document's audio back; holding
        // its decoded buffers would keep them resident for the rest of the session.
        self.bank = auris_core::AudioSourceBank::new();
        self.waveforms.clear();
        self.set_project(project);
        self.set_status("New project");
    }

    /// Saves to the current path, prompting when there is not one yet.
    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.project_path.clone() {
            Some(path) => self.write_project(&path),
            None => self.save_as(window, cx),
        }
    }

    /// Prompts for a path and saves there.
    pub(crate) fn save_as(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.project.name.clone();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Save project")
                .set_file_name(format!("{name}.{PROJECT_EXTENSION}"))
                .add_filter("Auris project", &[PROJECT_EXTENSION])
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                this.write_project(&path);
                cx.notify();
            });
        })
        .detach();
    }

    fn write_project(&mut self, path: &Path) {
        match save_project(path, &self.project) {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                self.dirty = false;
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(error) => self.set_status(format!("Could not save: {error}")),
        }
    }

    /// Prompts for a project file and opens it.
    pub(crate) fn open_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title("Open project")
                .add_filter("Auris project", &[PROJECT_EXTENSION])
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let loaded = load_project(&path);
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(project) => {
                        this.history.clear();
                        // Sample data lives outside the project file; the sources it references
                        // are reloaded lazily, so start from an empty bank rather than the
                        // previous document's audio.
                        this.bank = auris_core::AudioSourceBank::new();
                        this.waveforms.clear();
                        this.project_path = Some(path.clone());
                        this.dirty = false;
                        this.set_project(project);
                        let missing = this.reload_audio_sources();
                        // Fold the failures into the one status line rather than setting a
                        // message that the "Opened ..." below would immediately overwrite.
                        this.set_status(match missing.len() {
                            0 => format!("Opened {}", path.display()),
                            1 => format!(
                                "Opened {} — missing audio file {}",
                                path.display(),
                                missing[0].display()
                            ),
                            n => format!(
                                "Opened {} — {n} audio files could not be found",
                                path.display()
                            ),
                        });
                    }
                    Err(error) => this.set_status(format!("Could not open: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-decodes every audio source a freshly loaded project references.
    ///
    /// Returns the paths that could not be read, so the caller can report them in the single
    /// status line it sets rather than losing them behind it.
    fn reload_audio_sources(&mut self) -> Vec<PathBuf> {
        let sources: Vec<(auris_core::SourceId, PathBuf)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.clone()))
            .collect();
        let rate = self.project.sample_rate;
        let mut missing = Vec::new();
        for (id, path) in sources {
            match import_audio_file(&path, rate) {
                Ok(buffer) => self.install_audio_source(id, Arc::new(buffer)),
                Err(error) => {
                    log::warn!("could not reload {}: {error}", path.display());
                    missing.push(path);
                }
            }
        }
        self.rebuild_graph();
        missing
    }

    /// Prompts for an audio file and drops it onto a new audio track.
    pub(crate) fn import_audio(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let rate = self.project.sample_rate;
        let extensions: Vec<String> = supported_extensions()
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
                this.set_status(format!("Importing {}…", path.display()));
                cx.notify();
            });

            // Decoding a long file takes seconds; keep it off the UI thread.
            let decoded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { import_audio_file(&path, rate) }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match decoded {
                    Ok(buffer) => this.place_imported_audio(path, buffer),
                    Err(error) => this.set_status(format!("Could not import: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Registers decoded audio and puts a clip for it on a new track.
    fn place_imported_audio(&mut self, path: PathBuf, buffer: auris_core::AudioBuffer) {
        self.edit("Import audio");
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio".to_string());
        let source_id = self.project.add_audio_source(
            name.clone(),
            path,
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        let track = self.project.add_audio_track(name);
        let playhead = self.playhead_ticks();
        self.project.add_audio_clip(track, source_id, playhead);
        self.selected_track = Some(track);
        self.install_audio_source(source_id, Arc::new(buffer));
        self.rebuild_graph();
        self.set_status("Imported audio");
    }

    /// Stores decoded audio and computes the peaks used to draw it.
    fn install_audio_source(
        &mut self,
        id: auris_core::SourceId,
        buffer: Arc<auris_core::AudioBuffer>,
    ) {
        // One bucket per 256 frames keeps a five-minute stereo file's peak data under a
        // megabyte while still resolving individual drum hits at normal zoom levels.
        let peaks = auris_gpu::compute_peaks(self.gpu.as_deref(), &buffer, 256);
        self.waveforms.insert(id, Arc::new(peaks));
        self.bank.insert(id, buffer);
    }

    /// Prompts for a destination and renders the project to a WAV file.
    pub(crate) fn start_export(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.export.as_ref().is_some_and(|e| e.result.is_none()) {
            self.set_status("An export is already running");
            return;
        }
        let name = self.project.name.clone();
        let project = self.project.clone();
        let bank = self.bank.clone();
        let registry = Arc::clone(&self.registry);

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
                    let options = OfflineOptions::whole_project();
                    let buffer = render_project_with_progress(
                        &project,
                        &bank,
                        &registry,
                        &options,
                        &mut |fraction| {
                            progress.store(fraction.to_bits(), Ordering::Relaxed);
                        },
                    )
                    .map_err(|error| format!("render failed: {error}"))?;

                    let settings = WavExportSettings {
                        sample_rate: project.sample_rate.round() as u32,
                        ..WavExportSettings::default()
                    };
                    let peak = buffer.peak();
                    let seconds = buffer.duration_seconds();
                    write_wav(&render_path, &buffer, &settings)
                        .map_err(|error| format!("could not write the file: {error}"))?;
                    Ok::<(f64, f32), String>((seconds, peak))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let message = match rendered {
                    Ok((seconds, peak)) => {
                        let summary = format!(
                            "Exported {} · {} · peak {:.1} dBFS",
                            path.display(),
                            Seconds(seconds).format_clock(),
                            auris_core::param::gain_to_db(peak)
                        );
                        this.set_status(summary.clone());
                        Ok(summary)
                    }
                    Err(error) => {
                        let message = format!("Export failed: {error}");
                        this.set_status(message.clone());
                        Err(message)
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
}
