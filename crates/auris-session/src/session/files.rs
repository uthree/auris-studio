//! Everything that reaches the disk.
//!
//! Opening and saving a project, importing and exporting a Standard MIDI File, importing audio
//! and SoundFonts, and the shipped library. What these have in common is not a data structure but
//! a failure mode: a file can be gone, renamed or refused by the time it is asked for, so every
//! command here validates before it records an undo step and reports what it could not do rather
//! than logging it.
//!
//! Saving is a *folder*, not a file — see [`crate::guide::documents`] for the invariant that
//! makes that necessary, and [`Session::save_as_replacing`] for what happens when somebody aims
//! at a folder that already holds a different song.
//!
//! Finding a file the document names but that is no longer where it said is `assets`. The split
//! is between what the user asked for and what has to be true afterwards: [`Session::open`] is
//! here, and the two-pass search it ends with is there.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use auris_core::project::CurvePoint;
use auris_core::time::Ticks;
use auris_core::{
    AssetPath, AudioBuffer, ClipId, Note, Project, SoundFontId, SoundFontRef, SourceId,
};
use auris_io::{
    IoError, SoundFont, SoundFontPreset, byte_size, document_in_folder, font_name,
    import_audio_file, load_project, load_soundfont, preset_count, presets, save_project,
};

use crate::error::SessionError;
use crate::history::Edit;

use super::assets::{PreparedAssets, prepare_project_assets};
use super::{CachedFont, MidiReport, SaveReport, Session};

/// Decodes an audio file to `sample_rate`, without a session and without touching a document.
///
/// The slow half of [`Session::import_audio`], and the half that needs nothing but the file. A
/// frontend whose window would otherwise stop repainting for the length of the decode runs this on
/// a worker thread and hands what it gets to [`Session::place_audio`], on the thread that owns the
/// session — the only thread a document may be touched from.
pub fn decode_audio(path: &Path, sample_rate: f64) -> Result<AudioBuffer, SessionError> {
    Ok(import_audio_file(path, sample_rate)?)
}

/// A SoundFont read into memory and not yet given to a document.
///
/// Opaque on purpose. It exists to be carried from the thread that read it to the thread that owns
/// the session, and a frontend has no other use for one; what is inside belongs to the sampler,
/// and naming it here would put a sampler type in the signatures of every frontend.
pub struct LoadedFont(Arc<SoundFont>);

/// Reads a SoundFont into memory, without a session and without touching a document.
///
/// [`decode_audio`]'s counterpart and for its reason, only more so: a font is often hundreds of
/// megabytes, and reading one on the thread that draws is a window that stops answering for a
/// second or two with nothing to say for itself.
pub fn read_soundfont(path: &Path) -> Result<LoadedFont, SessionError> {
    Ok(LoadedFont(load_soundfont(path)?))
}

/// The instrument a track that played on the drum channel is given.
const DRUM_INSTRUMENT: &str = "auris.synth.noisedrum";

/// MIDI's drum channel, 0-based. Channel 10, counting the way a musician does.
const DRUM_CHANNEL: u8 = 9;

/// A MIDI import captured on the session thread and ready to parse on a worker.
///
/// The plugin choices and document revision are fixed before the worker starts. Running the job
/// never touches the live document; [`Session::continue_midi_import`] is the only applying step.
pub struct MidiImportJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    path: PathBuf,
    sample_rate: f64,
    fallback: String,
    drums: String,
}

/// A parsed MIDI document waiting for its short session-thread handoff.
pub struct MidiImportResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    project: Project,
    report: MidiReport,
}

/// A MIDI export snapshot that can be encoded and written away from the session thread.
///
/// The project and destination are fixed when the command starts. Running the job never reads
/// the live document; [`Session::continue_midi_export`] only decides whether its completion still
/// belongs to the document that requested it.
pub struct MidiExportJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    path: PathBuf,
    project: Project,
}

/// A completed MIDI export waiting for its short session-thread acknowledgement.
pub struct MidiExportResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    path: PathBuf,
    staged: auris_io::StagedMidi,
}

/// A project open captured for worker-side parsing and asset decoding.
pub struct OpenProjectJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    path: PathBuf,
    render_rate: f64,
}

/// A complete project and its decoded files waiting for a short session-thread handoff.
pub struct OpenProjectResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    path: PathBuf,
    project: Project,
    assets: PreparedAssets,
}

/// An archive copy plan captured without moving any bytes on the session thread.
pub struct CollectAssetsJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    folder: PathBuf,
    sources: Vec<(SourceId, PathBuf)>,
    fonts: Vec<(SoundFontId, PathBuf)>,
}

/// Files copied by a worker and waiting for their document references to be updated.
pub struct CollectAssetsResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    folder: PathBuf,
    copied_sources: Vec<(SourceId, PathBuf, PathBuf)>,
    copied_fonts: Vec<(SoundFontId, PathBuf, PathBuf)>,
    failed: Option<SessionError>,
}

/// A document snapshot prepared for a worker-side Save or Save As operation.
///
/// Native plug-in state is collected before this value is created. Running the job performs all
/// filesystem work without touching the live session; [`Session::continue_save`] is the only
/// point that adopts the saved location and snapshot.
pub struct SaveJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    original_path: Option<PathBuf>,
    document: PathBuf,
    project: Project,
    saved_project: Project,
    had_disk_fingerprint: bool,
    policy: SaveAsPolicy,
    collect_assets: bool,
    audio: Vec<(SourceId, Option<PathBuf>)>,
    fonts: Vec<(SoundFontId, Option<PathBuf>)>,
}

/// A durable document snapshot waiting for its short session-thread adoption.
pub struct SaveResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    original_path: Option<PathBuf>,
    document: PathBuf,
    project: Project,
    uncollected: Vec<PathBuf>,
    staged_checkpoint: Option<auris_io::StagedProject>,
    staged_document: auris_io::StagedProject,
    _lock: std::fs::File,
    disk_stamp: Option<std::time::SystemTime>,
    disk_fingerprint: u64,
}

impl CollectAssetsJob {
    /// Copies the planned files, reporting whole-operation progress between files.
    ///
    /// `None` means cancellation was observed. Copies already completed stay on disk but no live
    /// document reference changes until [`Session::continue_collect_assets`] accepts the result.
    pub fn run(
        self,
        cancelled: &AtomicBool,
        mut report: impl FnMut(f32),
    ) -> Option<CollectAssetsResult> {
        let total = self.sources.len() + self.fonts.len();
        let mut copied_sources = Vec::new();
        let mut copied_fonts = Vec::new();
        let mut failed = None;
        let mut completed = 0usize;
        let destination = self.folder.join(auris_io::AUDIO_DIR);
        for (id, from) in self.sources {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            match auris_io::copy_into(&from, &destination) {
                Ok(name) => copied_sources.push((id, from, name.into())),
                Err(error) => failed = failed.or(Some(SessionError::from(error))),
            }
            completed += 1;
            report(completed as f32 / total.max(1) as f32);
        }
        for (id, from) in self.fonts {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            match auris_io::copy_into(&from, &destination) {
                Ok(name) => copied_fonts.push((id, from, name.into())),
                Err(error) => failed = failed.or(Some(SessionError::from(error))),
            }
            completed += 1;
            report(completed as f32 / total.max(1) as f32);
        }
        if total == 0 {
            report(1.0);
        }
        (!cancelled.load(Ordering::Relaxed)).then_some(CollectAssetsResult {
            owner: self.owner,
            revision: self.revision,
            folder: self.folder,
            copied_sources,
            copied_fonts,
            failed,
        })
    }
}

impl OpenProjectJob {
    /// Loads the document and every available asset, returning `None` when cancelled.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<OpenProjectResult>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let project = load_project(&self.path)?;
        let folder = auris_io::project_folder(&self.path);
        let Some(assets) = prepare_project_assets(&project, folder, self.render_rate, cancelled)
        else {
            return Ok(None);
        };
        Ok(Some(OpenProjectResult {
            owner: self.owner,
            revision: self.revision,
            path: self.path,
            project,
            assets,
        }))
    }
}

impl MidiImportJob {
    /// Reads and builds the replacement document, returning `None` when cancellation was seen.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<MidiImportResult>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let imported = auris_io::read_midi_file(&self.path)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let name = self
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let mut project = Project::new(name, self.sample_rate);
        project.tempo_map = imported.tempo_map.clone();
        project.signatures = imported.signatures.clone();
        let mut report = MidiReport {
            tracks: 0,
            notes: 0,
            length: imported.end(),
        };

        for track in &imported.tracks {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let instrument = match track.channel {
                DRUM_CHANNEL => self.drums.clone(),
                _ => self.fallback.clone(),
            };
            let track_id = if track.channel == DRUM_CHANNEL {
                project.add_drum_track(&track.name, instrument)
            } else {
                project.add_instrument_track(&track.name, instrument)
            };
            let (Some(first), Some(last)) = (
                track.notes.iter().map(|note| note.start).min(),
                track.notes.iter().map(|note| note.end()).max(),
            ) else {
                continue;
            };
            let Some(clip_id) = project.add_midi_clip(
                track_id,
                &track.name,
                first,
                Ticks((last - first).raw().max(1)),
            ) else {
                continue;
            };
            if let Some(clip) = project.midi_clip_mut(clip_id) {
                clip.notes = track
                    .notes
                    .iter()
                    .map(|note| Note {
                        start: note.start - first,
                        ..note.clone()
                    })
                    .collect();
                let rebase = |points: &[CurvePoint]| -> Vec<CurvePoint> {
                    points
                        .iter()
                        .filter(|point| point.at >= first && point.at <= last)
                        .map(|point| CurvePoint {
                            at: point.at - first,
                            ..*point
                        })
                        .collect()
                };
                clip.bend = rebase(&track.bend);
                for (number, points) in &track.controllers {
                    let points = rebase(points);
                    if !points.is_empty() {
                        clip.controllers.insert(*number, points);
                    }
                }
                clip.length_is_explicit = true;
                report.notes += clip.notes.len();
            }
            report.tracks += 1;
        }
        project.validate_loop_expansion()?;
        Ok(Some(MidiImportResult {
            owner: self.owner,
            revision: self.revision,
            project,
            report,
        }))
    }
}

impl MidiExportJob {
    /// Encodes and synchronises a private sibling, unless cancellation was requested.
    ///
    /// MIDI encoding is one library call and cannot be interrupted safely half way through. The
    /// destination remains untouched throughout: cancellation observed after encoding drops the
    /// private file, and [`Session::continue_midi_export`] alone can publish it after revalidation.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<MidiExportResult>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let staged = auris_io::stage_midi_file(&self.path, &self.project)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(MidiExportResult {
            owner: self.owner,
            revision: self.revision,
            path: self.path,
            staged,
        }))
    }
}

impl SaveJob {
    /// Performs the slow half of saving while holding the cross-process project lock.
    ///
    /// The visible document is never replaced here. Cancellation or dropping the returned result
    /// removes the private staged document, while [`Session::continue_save`] performs the atomic
    /// publication only after the originating session is revalidated.
    pub fn run(mut self, cancelled: &AtomicBool) -> Result<Option<SaveResult>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let folder = auris_io::project_folder(&self.document)
            .ok_or(SessionError::NoPath)?
            .to_path_buf();
        std::fs::create_dir_all(&folder).map_err(|source| IoError::Filesystem {
            path: folder.clone(),
            source,
        })?;
        let lock = project_write_lock(&self.document)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }

        if self.collect_assets {
            if self.policy == SaveAsPolicy::RefuseReplacement
                && self.document.exists()
                && self.original_path.as_deref() != Some(self.document.as_path())
            {
                return Err(SessionError::WouldReplace(self.document));
            }
        } else if (self.document.exists() || self.had_disk_fingerprint)
            && load_project(&self.document)? != self.saved_project
        {
            return Err(SessionError::ExternalChanges(self.document));
        }

        let staged_checkpoint = if self.collect_assets && self.document.is_file() {
            let previous = load_project(&self.document)?;
            Some(super::checkpoints::stage_preserved_document_in_folder(&folder, &previous)?.1)
        } else {
            None
        };

        let destination = folder.join(auris_io::AUDIO_DIR);
        let mut uncollected = Vec::new();
        if self.collect_assets {
            for (id, from) in self.audio {
                let Some(from) = from else { continue };
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                match auris_io::copy_into(&from, &destination) {
                    Ok(name) => {
                        if let Some(source) = self.project.audio_sources.get_mut(&id) {
                            source.path =
                                AssetPath::inside(Path::new(auris_io::AUDIO_DIR).join(&name));
                            source.byte_size = byte_size(&from);
                        }
                    }
                    Err(error) => {
                        log::warn!("could not collect {}: {error}", from.display());
                        if let Some(source) = self.project.audio_sources.get_mut(&id) {
                            source.path = AssetPath::external(&from);
                        }
                        uncollected.push(from);
                    }
                }
            }
            for (id, from) in self.fonts {
                let Some(from) = from else { continue };
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                match auris_io::copy_into(&from, &destination) {
                    Ok(name) => {
                        let collected = Path::new(auris_io::AUDIO_DIR).join(name);
                        if let Some(previous) = self
                            .project
                            .soundfonts
                            .get(&id)
                            .map(|font| font.path.clone())
                        {
                            relocate_composed_font_in_project(
                                &mut self.project,
                                &previous,
                                &AssetPath::inside(&collected),
                            );
                        }
                        if let Some(font) = self.project.soundfonts.get_mut(&id) {
                            font.path = AssetPath::inside(collected);
                        }
                    }
                    Err(error) => {
                        log::warn!("could not collect {}: {error}", from.display());
                        if let Some(previous) = self
                            .project
                            .soundfonts
                            .get(&id)
                            .map(|font| font.path.clone())
                        {
                            relocate_composed_font_in_project(
                                &mut self.project,
                                &previous,
                                &AssetPath::external(&from),
                            );
                        }
                        if let Some(font) = self.project.soundfonts.get_mut(&id) {
                            font.path = AssetPath::external(&from);
                        }
                        uncollected.push(from);
                    }
                }
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let staged_document = auris_io::stage_project(&self.document, &mut self.project)?;
        let disk_stamp = staged_document.modified();
        let disk_fingerprint = staged_document.fingerprint();
        Ok(Some(SaveResult {
            owner: self.owner,
            revision: self.revision,
            original_path: self.original_path,
            document: self.document,
            project: self.project,
            uncollected,
            staged_checkpoint,
            staged_document,
            _lock: lock,
            disk_stamp,
            disk_fingerprint,
        }))
    }
}

impl Session {
    /// Keeps a song picker's decoded font ready without importing it into the open document.
    ///
    /// The caller reads it off-thread with [`read_soundfont`]. Cancellation leaves the document,
    /// undo history and library references unchanged; composing later allocates the real font id.
    pub fn cache_song_soundfont(
        &mut self,
        path: &Path,
        font: LoadedFont,
    ) -> (String, Vec<SoundFontPreset>) {
        let name = font_name(&font.0, path);
        let presets = presets(&font.0);
        self.cache_font(path, font.0, false);
        (name, presets)
    }

    /// Replaces the document with an empty project holding one instrument track.
    pub fn new_project(&mut self) {
        self.sound_scope = crate::transient_id::transient_id("session");
        let mut project = Project::new("Untitled", self.project.sample_rate);
        if let Some(instrument) = self.registry.default_instrument_id() {
            project.add_instrument_track("Track 1", instrument);
        }
        self.history.clear();
        self.path = None;
        self.dirty = false;
        // History is cleared, so nothing can bring the old document's audio back; keeping the
        // decoded buffers would hold them for the rest of the process.
        self.clear_sources();
        self.replace_project(project);
        self.install_shipped_fonts();
        self.mark_saved();
    }

    /// Reads a Standard MIDI File as a new document.
    ///
    /// A new document rather than tracks added to this one, because a MIDI file carries its own
    /// tempo and meter: dropping its notes into a piece running at a different speed would give
    /// you the right notes at the wrong lengths, and there would be no way to tell from looking.
    /// The caller deals with unsaved work first, exactly as it does for an opened project — this
    /// clears the history and the path, so the imported piece has to be saved somewhere new
    /// rather than over the `.auris` that happened to be open.
    ///
    /// A track that played on **channel 10** gets the noise-drum instrument where the registry has
    /// one. It is the only thing a bare MIDI file says about what a track is *for*, and a General
    /// MIDI drum part played on a lead synth is not something anyone would keep.
    pub fn import_midi(&mut self, path: &Path) -> Result<MidiReport, SessionError> {
        let cancelled = AtomicBool::new(false);
        let result = self
            .begin_midi_import(path)?
            .run(&cancelled)?
            .expect("a local import is not cancelled");
        Ok(self
            .continue_midi_import(result)
            .expect("a local import keeps the same session revision"))
    }

    /// Captures a MIDI import for worker execution without reading the file.
    pub fn begin_midi_import(&self, path: &Path) -> Result<MidiImportJob, SessionError> {
        let fallback = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        let drums = match self.registry.has_instrument(DRUM_INSTRUMENT) {
            true => DRUM_INSTRUMENT.to_string(),
            false => fallback.clone(),
        };
        Ok(MidiImportJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            path: path.to_path_buf(),
            sample_rate: self.project.sample_rate,
            fallback,
            drums,
        })
    }

    /// Applies a prepared MIDI document if the originating session is still unchanged.
    ///
    /// `None` is a stale result. The prepared document is dropped without touching the current
    /// project, path, history, or decoded sources.
    pub fn continue_midi_import(&mut self, result: MidiImportResult) -> Option<MidiReport> {
        if !Arc::ptr_eq(&self.registry, &result.owner) || self.revision != result.revision {
            return None;
        }
        self.history.clear();
        self.sound_scope = crate::transient_id::transient_id("session");
        self.path = None;
        self.clear_sources();
        self.replace_project(result.project);
        self.install_shipped_fonts();
        // Dirty from the first frame: nothing on disk holds this document, and the `.mid` it came
        // from cannot hold it either.
        self.dirty = true;
        Some(result.report)
    }

    /// Writes the open document's instrument tracks as a Standard MIDI File.
    ///
    /// Returns how many notes were written. What a `.mid` has nowhere to put — audio tracks, the
    /// mixer, which instrument each track plays, the automation — is left behind; see
    /// [`auris_io::midi`] for the whole list.
    pub fn export_midi(&self, path: &Path) -> Result<usize, SessionError> {
        Ok(auris_io::write_midi_file(path, &self.project)?)
    }

    /// Encodes and synchronises a private MIDI sibling for a caller-managed commit.
    ///
    /// Model-facing commands use this to put their cancellation boundary immediately before a
    /// no-replace publication. Dropping the result leaves `path` untouched.
    pub fn stage_midi_export(&self, path: &Path) -> Result<auris_io::StagedMidi, SessionError> {
        Ok(auris_io::stage_midi_file(path, &self.project)?)
    }

    /// Captures the open document for a worker-side MIDI export.
    pub fn begin_midi_export(&self, path: &Path) -> MidiExportJob {
        MidiExportJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            path: path.to_path_buf(),
            project: self.project.clone(),
        }
    }

    /// Publishes a worker export if it still belongs to this unchanged, uncancelled command.
    ///
    /// `Ok(None)` means the result became stale or cancellation won before the commit. In either
    /// case dropping the private sibling leaves an existing destination byte-for-byte unchanged.
    /// A publication failure is returned and likewise keeps the previous destination intact.
    pub fn continue_midi_export(
        &self,
        result: MidiExportResult,
        cancelled: &AtomicBool,
    ) -> Result<Option<(PathBuf, usize)>, SessionError> {
        if cancelled.load(Ordering::Relaxed)
            || !Arc::ptr_eq(&self.registry, &result.owner)
            || self.revision != result.revision
        {
            return Ok(None);
        }
        let notes = result.staged.publish()?;
        Ok(Some((result.path, notes)))
    }

    /// The folder relative asset paths resolve against.
    ///
    /// Before the user chooses a permanent project path this is the session's private working
    /// folder. Generated vocals, recordings and imported assets can therefore be used immediately;
    /// [`Self::save_as`] later collects them into the chosen project folder. Use [`Self::path`]
    /// when the distinction between a saved and an unsaved document matters.
    pub fn project_folder(&self) -> Option<&Path> {
        self.path
            .as_deref()
            .and_then(auris_io::project_folder)
            .or_else(|| Some(self.work_dir.path()))
    }

    /// The build that saved the open document, when it was not this one.
    ///
    /// `None` for a document this build saved, and for one never saved at all. `Some("")` is a
    /// file from before the record existed — an older build by definition, just one that left no
    /// name. This is the cue for the door-side note the guide's contract asks for: the text of
    /// this document is exactly as saved, but regenerating any of it is a redraw in the current
    /// composer's style, so a take worth keeping wants freezing before the button.
    pub fn saved_by_another_build(&self) -> Option<&str> {
        self.path.as_ref()?;
        let stored = self.project.saved_by.as_str();
        (stored != env!("CARGO_PKG_VERSION")).then_some(stored)
    }

    /// Opens a project file.
    ///
    /// Returns the references that could not be found — audio files and SoundFonts alike. The
    /// project still opens; whatever named them is silent until the files come back, which is
    /// far friendlier than refusing to open a session because one sample moved.
    ///
    /// A file that has moved but can still be found is written back into the document under its
    /// new reference, which leaves the project dirty. That is the point: the search happens once,
    /// and saving makes the repair permanent.
    pub fn open(&mut self, path: &Path) -> Result<Vec<PathBuf>, SessionError> {
        let project = load_project(path)?;
        self.history.clear();
        self.clear_sources();
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.mark_saved();
        self.saved_project = project.clone();
        self.saved_edit_project = project.clone();
        // Hosted plugins belong to the document that named them: their slot ids come from it,
        // and this document reusing an id would inherit the old plugin. The loaded *files* are
        // kept — a `.clap` is the same code whichever project is open.
        self.hosted.clear();
        self.vst3.clear();
        // Arms and monitors name bare track ids too. A colliding id in another document must not
        // silently inherit a device binding made for the previous track.
        self.armed.clear();
        self.monitored.clear();
        self.publish_monitors();
        self.close_input_if_idle();
        self.adopt_project(project);

        let missing = self.reload_assets();
        // After the search, not before it. A project saved on another machine names the shipped
        // font at *that* machine's path; the search finds this machine's copy and writes the new
        // path into the document, and only then does the id it already has match the file about
        // to be installed. The other way round, the same font would arrive twice under two ids —
        // and be held in memory twice, which for this font is four hundred megabytes.
        self.install_shipped_fonts();
        self.rebuild_graph();
        // After the rebuild, because a hosted plugin cannot say where its parameters are until it
        // has been placed — and again afterwards when a lane moved, since the graph resolved the
        // lanes to positions that have just changed underneath it.
        if self.realign_automation() {
            self.rebuild_graph();
        }
        // The document was adopted without telling the engine, so the loop it holds is still the
        // one the *previous* project had.
        self.publish_loop();
        Ok(missing)
    }

    /// Captures a project open without reading the document or any of its assets.
    pub fn begin_open_project(&self, path: &Path) -> OpenProjectJob {
        OpenProjectJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            path: path.to_path_buf(),
            render_rate: self.engine.sample_rate(),
        }
    }

    /// Applies a worker-loaded project if the originating session is still unchanged.
    ///
    /// `None` is a stale result and leaves all document, history, asset, and path state alone.
    pub fn continue_open_project(&mut self, result: OpenProjectResult) -> Option<Vec<PathBuf>> {
        if !Arc::ptr_eq(&self.registry, &result.owner)
            || self.revision != result.revision
            || self.engine.sample_rate() != result.assets.render_rate
        {
            return None;
        }
        self.history.clear();
        self.clear_sources();
        self.path = Some(result.path);
        self.dirty = false;
        self.mark_saved();
        self.saved_project = result.project.clone();
        self.saved_edit_project = result.project.clone();
        self.hosted.clear();
        self.vst3.clear();
        self.armed.clear();
        self.monitored.clear();
        self.publish_monitors();
        self.close_input_if_idle();
        self.adopt_project(result.project);
        let missing = self.install_prepared_assets(result.assets);
        self.install_shipped_fonts();
        self.rebuild_graph();
        if self.realign_automation() {
            self.rebuild_graph();
        }
        self.publish_loop();
        Some(missing)
    }

    /// Writes the document at exactly `path`, without moving or collecting anything.
    ///
    /// The project folder becomes the directory holding `path`, so a caller choosing a fresh
    /// location wants [`Self::save_as`] instead — this one would leave the audio behind.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        self.collect_hosted_state()?;
        save_project(path, &mut self.project)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        // Every write restarts the autosave clock, so saving by hand postpones the next automatic
        // one rather than being followed by a second write a moment later.
        self.mark_saved();
        Ok(())
    }

    /// Saves to a new location, creating the project folder and collecting the audio into it.
    ///
    /// `chosen` is whatever a save dialog returned; the document lands at the path this returns,
    /// which is `chosen` placed in a folder of its own. The audio the project owns is copied in
    /// alongside it, so the folder can afterwards be moved, renamed, copied to another machine or
    /// zipped up and still open.
    ///
    /// SoundFonts outside the folder are left where they are. A font is a library shared by every
    /// project that uses it, and a copy per project would cost gigabytes to save a path;
    /// [`Self::collect_assets`] is how someone archiving a project asks for those too. A font
    /// already *inside* the folder is a file this project owns like any other, and travels.
    pub fn save_as(&mut self, chosen: &Path) -> Result<SaveReport, SessionError> {
        let result = self
            .begin_save_as(chosen)?
            .run(&AtomicBool::new(false))?
            .expect("a local save is not cancelled");
        self.continue_save(result)
            .expect("a local save keeps the same session identity and revision")
    }

    /// [`Self::save_as`] with the replacement already agreed to.
    ///
    /// For a host that has shown the user which project is about to be overwritten and been told
    /// to go ahead. Nothing else differs.
    pub fn save_as_replacing(&mut self, chosen: &Path) -> Result<SaveReport, SessionError> {
        let result = self
            .begin_save_as_replacing(chosen)?
            .run(&AtomicBool::new(false))?
            .expect("a local save is not cancelled");
        self.continue_save(result)
            .expect("a local save keeps the same session identity and revision")
    }

    /// Captures a Save As request without touching its destination.
    pub fn begin_save_as(&mut self, chosen: &Path) -> Result<SaveJob, SessionError> {
        self.begin_save_as_with_policy(chosen, SaveAsPolicy::RefuseReplacement)
    }

    /// Captures a confirmed replacement Save As request without touching its destination.
    pub fn begin_save_as_replacing(&mut self, chosen: &Path) -> Result<SaveJob, SessionError> {
        self.begin_save_as_with_policy(chosen, SaveAsPolicy::Replace)
    }

    fn begin_save_as_with_policy(
        &mut self,
        chosen: &Path,
        policy: SaveAsPolicy,
    ) -> Result<SaveJob, SessionError> {
        let document = document_in_folder(chosen);
        self.collect_hosted_state()?;
        // Resolve before the document moves: an `Inside` reference read against the new folder
        // would point at a file that has not been copied there yet.
        let audio = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.resolve(self.project_folder())))
            .collect();
        // A font is left where it lies, and an external one stays external — Save As is not the
        // archiving opt-in. But a font that is already `Inside` lives in the *old* folder, and
        // carrying its reference across unchanged would leave the copy naming a file that is not
        // there: the save would report success, playback here would go on sounding from the
        // samples already in memory, and every track on that font would open silent elsewhere.
        let fonts = self
            .project
            .soundfonts
            .values()
            .filter(|font| font.path.is_inside())
            .map(|font| (font.id, font.path.resolve(self.project_folder())))
            .collect();
        Ok(SaveJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            original_path: self.path.clone(),
            document,
            project: self.project.clone(),
            saved_project: self.saved_project.clone(),
            had_disk_fingerprint: self.disk_fingerprint.is_some(),
            policy,
            collect_assets: true,
            audio,
            fonts,
        })
    }

    /// Saves to the path the project was last saved to or opened from.
    pub fn save_in_place(&mut self) -> Result<(), SessionError> {
        let result = self
            .begin_save_in_place()?
            .run(&AtomicBool::new(false))?
            .expect("a local save is not cancelled");
        self.continue_save(result)
            .expect("a local save keeps the same session identity and revision")
            .map(drop)
    }

    /// Captures an in-place Save without reading or writing its document.
    pub fn begin_save_in_place(&mut self) -> Result<SaveJob, SessionError> {
        let document = self.path.clone().ok_or(SessionError::NoPath)?;
        self.collect_hosted_state()?;
        Ok(SaveJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            original_path: self.path.clone(),
            document,
            project: self.project.clone(),
            saved_project: self.saved_project.clone(),
            had_disk_fingerprint: self.disk_fingerprint.is_some(),
            policy: SaveAsPolicy::RefuseReplacement,
            collect_assets: false,
            audio: Vec::new(),
            fonts: Vec::new(),
        })
    }

    /// Publishes and adopts a prepared save if its originating document is still unchanged.
    ///
    /// `None` is stale. In that case the staged document is dropped and the visible destination
    /// remains untouched.
    pub fn continue_save(
        &mut self,
        result: SaveResult,
    ) -> Option<Result<SaveReport, SessionError>> {
        if !Arc::ptr_eq(&self.registry, &result.owner)
            || self.revision != result.revision
            || self.path != result.original_path
        {
            return None;
        }
        let SaveResult {
            document,
            project,
            uncollected,
            staged_checkpoint,
            staged_document,
            _lock,
            disk_stamp,
            disk_fingerprint,
            ..
        } = result;
        if let Some(checkpoint) = staged_checkpoint
            && let Err(error) = checkpoint.publish()
        {
            return Some(Err(error.into()));
        }
        if let Err(error) = staged_document.publish() {
            return Some(Err(error.into()));
        }
        drop(_lock);

        self.project = project;
        self.path = Some(document.clone());
        let cached_fonts: Vec<_> = self
            .project
            .soundfonts
            .values()
            .filter_map(|font| {
                let samples = self.fonts.get(font.id)?;
                let path = font.path.resolve(self.project_folder())?;
                Some((path, samples))
            })
            .collect();
        for (path, samples) in cached_fonts {
            self.cache_font(&path, samples, false);
        }
        self.dirty = false;
        self.mark_saved_from_worker(disk_stamp, disk_fingerprint);
        Some(Ok(SaveReport {
            document,
            uncollected,
        }))
    }

    /// Accepts the open document's disk version as one undoable edit.
    ///
    /// Local unsaved changes remain on the undo stack. Callers should offer this explicitly
    /// when the window is dirty; an active gesture is always refused. Missing assets are
    /// reported exactly as by [`Self::open`].
    pub fn reload_external_changes(&mut self) -> Result<Vec<PathBuf>, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        let project = load_project(&path)?;
        if project == self.project {
            self.mark_saved();
            self.dirty = false;
            return Ok(Vec::new());
        }
        self.record(crate::Edit::ExternalChanges);
        let saved = project.clone();
        self.saved_edit_project = saved.clone();
        let missing = self.replace_external_project(project);
        let saved_edit = self.saved_edit_project.clone();
        self.mark_saved();
        self.saved_project = saved;
        self.saved_edit_project = saved_edit;
        self.dirty = self.project != self.saved_edit_project;
        Ok(missing)
    }

    /// Rebuilds file-backed state while keeping assets referenced by older undo entries alive.
    pub(super) fn replace_external_project(&mut self, project: Project) -> Vec<PathBuf> {
        self.hosted.clear();
        self.vst3.clear();
        self.adopt_project(project);
        let missing = self.reload_assets();
        self.install_shipped_fonts();
        self.rebuild_graph();
        if self.realign_automation() {
            self.rebuild_graph();
        }
        self.publish_loop();
        missing
    }

    /// Copies every loaded asset the project refers to into its folder, however large.
    ///
    /// The command for archiving a project or sending it to someone else: afterwards the folder
    /// holds everything, and nothing outside it is needed to open the project. Explicit rather
    /// than automatic because a SoundFont library runs to hundreds of megabytes per font, and
    /// paying that on every save to shorten a path nobody reads would be a poor trade.
    ///
    /// Returns how many files were copied in. Anything already inside is left alone, so running
    /// this twice costs a directory listing. A reference that did not decode as the audio or
    /// SoundFont it claims to be is left outside; a document received from elsewhere is not
    /// authority to copy an arbitrary local file into an archive.
    ///
    /// A file that cannot be copied is skipped and the first failure reported *after* every
    /// other file has had its attempt — missing assets are reported, never fatal, and what was
    /// copied stays copied and marked unsaved. A retry adopts what already landed.
    pub fn collect_assets(&mut self) -> Result<usize, SessionError> {
        // Each copy below finds the folder for itself. This is here so that a project which has
        // never been saved is told so, rather than being handed a cheerful `Ok(0)` for having
        // collected nothing into nowhere.
        if self.path.is_none() {
            return Err(SessionError::NoPath);
        }

        let sources: Vec<(SourceId, Option<PathBuf>)> = self
            .project
            .audio_sources
            .values()
            .filter(|source| !source.path.is_inside() && self.bank.get(source.id).is_some())
            .map(|source| (source.id, source.path.resolve(None)))
            .collect();
        let fonts: Vec<(SoundFontId, Option<PathBuf>)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| !font.path.is_inside() && self.fonts.contains(font.id))
            .map(|font| (font.id, font.path.resolve(None)))
            .collect();

        let mut collected = 0;
        let mut failed: Option<SessionError> = None;
        for (id, from) in sources {
            let Some(from) = from else { continue };
            match self.collect_source(id, &from) {
                Ok(()) => collected += 1,
                // Aborting here used to leave the rest uncopied and — worse — the documents
                // already rewritten to `Inside` unmarked, so a clean-looking session disagreed
                // with its own file.
                Err(error) => failed = failed.or(Some(error)),
            }
        }
        for (id, from) in fonts {
            let Some(from) = from else { continue };
            match self.collect_font(id, &from) {
                Ok(()) => collected += 1,
                Err(error) => failed = failed.or(Some(error)),
            }
        }

        if collected > 0 {
            self.dirty = true;
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(collected),
        }
    }

    /// Captures the external loaded assets that an archive command needs to copy.
    pub fn begin_collect_assets(&self) -> Result<CollectAssetsJob, SessionError> {
        if self.path.is_none() {
            return Err(SessionError::NoPath);
        }
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;
        let sources = self
            .project
            .audio_sources
            .values()
            .filter(|source| !source.path.is_inside() && self.bank.get(source.id).is_some())
            .filter_map(|source| source.path.resolve(None).map(|path| (source.id, path)))
            .collect();
        let fonts = self
            .project
            .soundfonts
            .values()
            .filter(|font| !font.path.is_inside() && self.fonts.contains(font.id))
            .filter_map(|font| font.path.resolve(None).map(|path| (font.id, path)))
            .collect();
        Ok(CollectAssetsJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            folder,
            sources,
            fonts,
        })
    }

    /// Adopts worker copies when the document still matches the capture.
    ///
    /// The outer `None` is stale. The inner error is the first failed copy after every successful
    /// reference has been made durable in the in-memory document, matching [`Self::collect_assets`].
    pub fn continue_collect_assets(
        &mut self,
        result: CollectAssetsResult,
    ) -> Option<Result<usize, SessionError>> {
        if !Arc::ptr_eq(&self.registry, &result.owner) || self.revision != result.revision {
            return None;
        }
        let CollectAssetsResult {
            folder,
            copied_sources,
            copied_fonts,
            failed,
            ..
        } = result;
        let mut collected = 0;
        for (id, from, name) in copied_sources {
            if let Some(source) = self.project.audio_sources.get_mut(&id) {
                source.path = AssetPath::inside(Path::new(auris_io::AUDIO_DIR).join(name));
                source.byte_size = byte_size(&from);
                collected += 1;
            }
        }
        for (id, _from, name) in copied_fonts {
            let collected_path = Path::new(auris_io::AUDIO_DIR).join(name);
            if let Some(previous) = self
                .project
                .soundfonts
                .get(&id)
                .map(|font| font.path.clone())
            {
                self.relocate_composed_font(&previous, &AssetPath::inside(&collected_path));
            }
            if let Some(font) = self.project.soundfonts.get_mut(&id) {
                font.path = AssetPath::inside(&collected_path);
                collected += 1;
            }
            if let Some(samples) = self.fonts.get(id) {
                self.cache_font(&folder.join(&collected_path), samples, false);
            }
        }
        if collected > 0 {
            self.revision = self.revision.wrapping_add(1);
            self.dirty = true;
        }
        Some(match failed {
            Some(error) => Err(error),
            None => Ok(collected),
        })
    }

    /// Imports an audio file, adds a track for it and places a clip at `start`.
    ///
    /// The file is copied into the project folder, so the song owns its own audio from the
    /// moment it is imported. A project that has not been saved yet has no folder to copy into
    /// and refers to the file where it lies; saving picks it up.
    pub fn import_audio(&mut self, path: &Path, start: Ticks) -> Result<ClipId, SessionError> {
        let buffer = decode_audio(path, self.project.sample_rate)?;
        self.place_audio(path, buffer, start)
    }

    /// Puts audio that has already been decoded on a track of its own, with a clip at `start`.
    ///
    /// [`Self::import_audio`]'s other half — everything it does that needs the document, and so
    /// everything that has to happen on the thread holding the session. `path` is still wanted
    /// here: it names the track and the source, and it is what the document refers to until the
    /// project has a folder to copy the file into.
    ///
    /// The buffer is expected at the project's sample rate, which is what [`decode_audio`] was
    /// given it for — and is read again here, because the whole point of the split is that
    /// something else can happen in between. A document whose rate moved while its audio was
    /// being decoded gets the decode again rather than a track playing at the wrong pitch; it
    /// costs a re-read of a file that is still warm, and only in the case that used to be
    /// impossible.
    pub fn place_audio(
        &mut self,
        path: &Path,
        buffer: AudioBuffer,
        start: Ticks,
    ) -> Result<ClipId, SessionError> {
        let buffer = match buffer.sample_rate() == self.project.sample_rate {
            true => buffer,
            false => decode_audio(path, self.project.sample_rate)?,
        };
        self.record(Edit::ImportAudio);
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio".to_string());
        let source = self.project.add_audio_source(
            name.clone(),
            AssetPath::external(path),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        // Written down now rather than after the copy below, because the copy is the branch that
        // may not happen: a project with no folder yet keeps pointing at the file where it lies,
        // and that reference is the one most likely to need finding again later.
        self.record_source_size(source, path);
        // A failure to copy is not a failure to import: the audio decoded, and referring to it
        // where it lies is exactly what an unsaved project does anyway.
        // The private working folder is for files this session creates. Imported audio remains
        // external until the user has chosen a permanent project folder, so Save As can apply the
        // normal collection policy and plain `save` keeps its deliberately low-level contract.
        let has_folder = self.path.is_some();
        if has_folder && let Err(error) = self.collect_source(source, path) {
            log::warn!("could not collect {}: {error}", path.display());
        }
        let track = self.project.add_audio_track(name);
        let clip = self
            .project
            .add_audio_clip(track, source, start.max_zero())
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();
        Ok(clip)
    }

    /// Imports a SoundFont, making its sounds available to every track in the project.
    ///
    /// The file is referred to where it lies rather than copied in — see [`Self::save_as`] for
    /// why — so what the document records is enough to recognise it again: the path, and the
    /// size that tells the font which moved from a different one wearing its name.
    ///
    /// Importing the same file twice returns the id it already has, so a second attempt costs
    /// nothing and, more to the point, does not put a second copy of a very large object in
    /// memory. Nothing is heard until a track is pointed at one of its presets with
    /// [`Self::set_track_preset`].
    pub fn import_soundfont(&mut self, path: &Path) -> Result<SoundFontId, SessionError> {
        let font = read_soundfont(path)?;
        self.install_soundfont(path, font)
    }

    /// Puts a SoundFont that has already been read into the document's library.
    ///
    /// [`Self::import_soundfont`]'s other half, split from the read for [`Self::place_audio`]'s
    /// reason. Everything that makes re-importing cheap is here rather than in the read, so a
    /// frontend cannot skip it: a font the document already knows is not an undo step, and the
    /// samples are replaced rather than added to.
    pub fn install_soundfont(
        &mut self,
        path: &Path,
        font: LoadedFont,
    ) -> Result<SoundFontId, SessionError> {
        let font = font.0;
        let name = font_name(&font, path);
        let id = match self.project.soundfont_at(self.project_folder(), path) {
            Some(existing) => existing,
            None => {
                // Only a font the document does not know is an edit. Re-importing a known one
                // reloads its samples — which changes what is heard, not what is saved — and a
                // step for it would clear the redo stack over a document that did not move.
                self.record(Edit::ImportSoundFont);
                self.project
                    .add_soundfont(name, AssetPath::external(path), byte_size(path))
            }
        };
        self.cache_font(path, Arc::clone(&font), false);
        self.fonts.insert(id, font);
        // Fonts the document names but could not find may well be siblings of the one that was
        // just located by hand. Fixing one is then enough to fix the rest.
        if let Some(directory) = path.parent() {
            self.recover_fonts_from(directory);
        }
        // A track already naming this font — one whose file was missing when the project
        // opened, and which the user has just gone and found — starts sounding again.
        self.invalidate_graph();
        Ok(id)
    }

    /// Makes an already-read built-in SoundFont available without recording a user edit.
    ///
    /// A background download can finish during a gesture or after Undo. Its library reference
    /// therefore joins every undo/redo snapshot and the gesture's starting state as well as the
    /// current document. The dirty flag is preserved, and loaded samples are cached for later
    /// documents. A missing reference with the same filename and size keeps its existing id.
    pub fn install_shipped_soundfont(&mut self, path: &Path, font: LoadedFont) -> SoundFontId {
        let id = self.adopt_loaded_shipped_font(path, font.0);
        self.revision = self.revision.wrapping_add(1);
        // Rebuild even during a gesture: ending an otherwise unchanged gesture discards its
        // deferred rebuild, but this font can restore an already-playing track immediately.
        self.rebuild_graph();
        id
    }

    /// Shares library availability with snapshots; the caller decides when to rebuild.
    fn adopt_loaded_shipped_font(&mut self, path: &Path, font: Arc<SoundFont>) -> SoundFontId {
        self.cache_font(path, Arc::clone(&font), true);
        let folder = self.project_folder().map(Path::to_path_buf);
        let name = font_name(&font, path);
        let size = byte_size(path);

        // Reserve above every branch, including Redo: the live counter may have gone backwards
        // with Undo. Probing a clone leaves every document's allocation state untouched.
        let mut next = self.project.clone().next_effect_slot_id().0;
        let mut inspect = |project: &mut Project| {
            next = next.max(project.clone().next_effect_slot_id().0);
        };
        self.history.for_each_project_mut(&mut inspect);
        inspect(&mut self.saved_edit_project);
        if let Some(transaction) = &mut self.transaction {
            inspect(&mut transaction.before);
        }
        let reserve = SoundFontId(next);
        let install = |project: &mut Project| {
            install_library_reference(project, folder.as_deref(), path, &name, size, reserve)
        };
        let ids = install(&mut self.project);
        for id in &ids {
            self.fonts.insert(*id, Arc::clone(&font));
        }
        self.history.for_each_project_mut(|project| {
            install(project);
        });
        install(&mut self.saved_edit_project);
        if let Some(transaction) = &mut self.transaction {
            install(&mut transaction.before);
        }
        ids[0]
    }

    /// Puts the SoundFonts the application ships with into the document, so their sounds are in
    /// the library from the moment a project opens.
    ///
    /// Called wherever a document is created or opened rather than once at start-up, because a
    /// document is what holds the reference and every new one needs its own.
    ///
    /// Not an edit. The built-in instruments are not in the history either, and for the same
    /// reason: they are what this installation *has*, not something the user did. So no undo step
    /// is recorded, the dirty flag is left exactly as it was, and a new document that has only
    /// ever been looked at still counts as unmodified.
    ///
    /// A font already in the document under the same path keeps its id, which is what makes this
    /// safe on a project that was saved with one.
    ///
    /// Only files already available on disk are read here. A font acquired later arrives through
    /// [`Self::install_shipped_soundfont`].
    pub(super) fn install_shipped_fonts(&mut self) {
        if !self.shipped_library {
            return;
        }
        let dirty = self.dirty;
        let mut installed_any = false;
        for (font, path) in crate::library::installed_fonts() {
            match self.adopt_font(&path) {
                Some(_) => installed_any = true,
                None => log::warn!("could not read the shipped {}", font.name),
            }
        }
        self.dirty = dirty;
        if installed_any {
            self.invalidate_graph();
        }
    }

    /// Reads a font from the shipped library into the document without recording an edit.
    ///
    /// The samples are cached by path in `font_cache`, so the second call — after a
    /// **File → New**, which empties the id-keyed bank — costs a map lookup rather than two
    /// hundred megabytes of file.
    fn adopt_font(&mut self, path: &Path) -> Option<SoundFontId> {
        let font = self.shipped_font_data(path)?;
        Some(self.adopt_loaded_shipped_font(path, font))
    }

    /// A shipped font's samples, read from the file the first time and cached after that.
    fn shipped_font_data(&mut self, path: &Path) -> Option<Arc<SoundFont>> {
        if let Some(font) = self.font_cache.get(path) {
            return Some(Arc::clone(&font.samples));
        }
        let font = load_soundfont(path)
            .inspect_err(|error| log::warn!("{}: {error}", path.display()))
            .ok()?;
        self.cache_font(path, Arc::clone(&font), true);
        Some(font)
    }

    /// Keeps the file identity alongside an id that a different history branch may reuse.
    pub(super) fn cache_font(&mut self, path: &Path, samples: Arc<SoundFont>, shipped: bool) {
        let shipped = shipped || self.font_cache.get(path).is_some_and(|font| font.shipped);
        self.font_cache
            .insert(path.to_path_buf(), CachedFont { samples, shipped });
    }

    /// Restores the current snapshot's font identities before its graph is rebuilt.
    pub(super) fn restore_cached_fonts(&mut self) {
        for reference in self.project.soundfonts.values() {
            let Some(path) = reference.path.resolve(self.project_folder()) else {
                continue;
            };
            if let Some(font) = self.font_cache.get(&path) {
                self.fonts.insert(reference.id, Arc::clone(&font.samples));
            } else {
                self.fonts.remove(reference.id);
            }
        }
    }

    /// Puts the shipped General MIDI font into a project being built, and returns its new id.
    ///
    /// Takes the project rather than working on [`Self::project`] because the only caller is
    /// [`Self::compose`], which assembles a whole document before it swaps one in — a font added
    /// to the open project would belong to the piece being replaced.
    ///
    /// `None` when nothing is installed, which is what makes a part asking for a violin come out
    /// as the oscillator it also names rather than as silence.
    pub(super) fn adopt_general_midi(&mut self, project: &mut Project) -> Option<SoundFontId> {
        if !self.shipped_library {
            return None;
        }
        let font = crate::library::shipped(crate::library::GENERAL_MIDI)?;
        let path = crate::library::installed(font)?;
        let data = self.shipped_font_data(&path)?;
        let name = font_name(&data, &path);
        let id = project.add_soundfont(name, AssetPath::external(&path), byte_size(&path));
        // Into the bank now rather than when the document is swapped in. The swap rebuilds the
        // graph, and a graph built while the samples were still missing would log a warning per
        // track about a font that is right here, then be thrown away and built again.
        self.fonts.insert(id, data);
        Some(id)
    }

    /// The same font, into the document that is already open.
    ///
    /// [`Self::accompany`] is the caller: it adds parts *beside* what a person has written rather
    /// than replacing the document, so its tracks need a font the open project names. Through
    /// [`Self::adopt_font`], so pressing it twice finds the font already there instead of writing
    /// a second reference to the same two hundred megabytes.
    pub(super) fn adopt_general_midi_here(&mut self) -> Option<SoundFontId> {
        if !self.shipped_library {
            return None;
        }
        let font = crate::library::shipped(crate::library::GENERAL_MIDI)?;
        let path = crate::library::installed(font)?;
        self.adopt_font(&path)
    }

    /// Every SoundFont the project knows about, whether or not its file is still there.
    pub fn soundfonts(&self) -> impl Iterator<Item = &SoundFontRef> {
        self.project.soundfonts.values()
    }

    /// `true` when a font's samples are actually in memory, so a track naming it will sound.
    pub fn soundfont_is_loaded(&self, id: SoundFontId) -> bool {
        self.fonts.contains(id)
    }

    /// How many sounds an imported font offers, without building the list.
    pub fn soundfont_preset_count(&self, id: SoundFontId) -> usize {
        self.fonts
            .get(id)
            .map(|font| preset_count(&font))
            .unwrap_or(0)
    }

    /// Every sound one imported font offers, in bank and patch order.
    ///
    /// Empty for a font whose file could not be read, which is the same thing a font with no
    /// presets would give — and a library showing nothing under a name is already the message.
    pub fn soundfont_presets(&self, id: SoundFontId) -> Vec<SoundFontPreset> {
        self.fonts
            .get(id)
            .map(|font| presets(&font))
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SaveAsPolicy {
    RefuseReplacement,
    Replace,
}

fn relocate_composed_font_in_project(project: &mut Project, from: &AssetPath, to: &AssetPath) {
    let Some(text) = project.song_spec.as_ref() else {
        return;
    };
    let Ok(mut spec) = auris_compose::SongSpec::parse(text) else {
        return;
    };
    let mut changed = false;
    for part in &mut spec.parts {
        if let Some(auris_compose::PartSource::SoundFont { path, .. }) = &mut part.source
            && path.as_path() == from.as_stored()
        {
            *path = to.as_stored().to_path_buf();
            changed = true;
        }
    }
    if changed {
        project.song_spec = Some(spec.to_toml());
    }
}

/// Adds library availability to one snapshot while preserving references the document owns.
fn install_library_reference(
    project: &mut Project,
    folder: Option<&Path>,
    path: &Path,
    name: &str,
    size: u64,
    reserve: SoundFontId,
) -> Vec<SoundFontId> {
    let mut ids = Vec::new();
    for reference in project.soundfonts.values_mut() {
        let resolved = reference.path.resolve(folder);
        if resolved.as_deref() == Some(path) {
            ids.push(reference.id);
        } else if reference.path.file_name() == path.file_name()
            && (reference.byte_size == 0 || reference.byte_size == size)
            && !resolved.is_some_and(|file| file.is_file())
        {
            reference.path = AssetPath::external(path);
            reference.byte_size = size;
            ids.push(reference.id);
        }
    }
    if ids.is_empty() {
        project.soundfonts.insert(
            reserve,
            SoundFontRef {
                id: reserve,
                name: name.to_string(),
                path: AssetPath::external(path),
                byte_size: size,
            },
        );
        project.repair_id_counter();
        ids.push(reserve);
    }
    ids
}

/// Serialises cooperating writers across processes, including the optimistic disk check.
fn project_write_lock(path: &Path) -> Result<std::fs::File, SessionError> {
    let folder = path.parent().ok_or(SessionError::NoPath)?;
    let lock_path = folder.join(".auris-write.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| IoError::from_fs(&lock_path, error))?;
    file.lock()
        .map_err(|error| IoError::from_fs(&lock_path, error))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::*;
    use crate::session::fixtures::{Scratch, named_font, session};
    use auris_io::AUDIO_DIR;

    fn directory_entries(path: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn detached_open_cancels_without_io_and_rejects_a_result_after_an_edit() {
        let scratch = Scratch::new("detached-open-stale");
        let document = scratch.join("Other.auris");
        let mut stored = session();
        stored.add_default_instrument_track("Stored").unwrap();
        stored.save(&document).unwrap();

        let mut live = session();
        live.add_default_instrument_track("Live").unwrap();
        let before_cancel = live.project().clone();
        let cancelled = AtomicBool::new(true);
        assert!(
            live.begin_open_project(&document)
                .run(&cancelled)
                .unwrap()
                .is_none()
        );
        assert_eq!(live.project(), &before_cancel);

        let job = live.begin_open_project(&document);
        live.add_default_instrument_track("Newer live edit")
            .unwrap();
        let edited = live.project().clone();
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(live.continue_open_project(result).is_none());
        assert_eq!(live.project(), &edited);
        assert!(
            live.can_undo(),
            "rejecting a stale open erased live history"
        );
    }

    #[test]
    fn detached_save_publishes_only_after_a_current_session_accepts_it() {
        let scratch = Scratch::new("detached-save-stale");
        let chosen = scratch.join("Song.auris");
        let mut live = session();
        live.add_default_instrument_track("Saved baseline").unwrap();
        let document = live.save_as(&chosen).unwrap().document;
        live.add_default_instrument_track("Captured edit").unwrap();
        let before = std::fs::read(&document).unwrap();

        let result = live
            .begin_save_in_place()
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(&document).unwrap(),
            before,
            "the worker must not publish its staged document"
        );

        live.add_default_instrument_track("Newer edit").unwrap();
        assert!(live.continue_save(result).is_none());
        assert_eq!(
            std::fs::read(&document).unwrap(),
            before,
            "rejecting stale work must leave the visible document untouched"
        );
        assert!(live.is_dirty());
    }

    #[test]
    fn detached_save_as_cancellation_does_not_create_a_document() {
        let scratch = Scratch::new("detached-save-cancel");
        let chosen = scratch.join("Cancelled.auris");
        let document = document_in_folder(&chosen);
        let mut live = session();
        live.add_default_instrument_track("Unsaved").unwrap();

        let cancelled = AtomicBool::new(true);
        assert!(
            live.begin_save_as(&chosen)
                .unwrap()
                .run(&cancelled)
                .unwrap()
                .is_none()
        );
        assert!(!document.exists());
        assert!(live.path().is_none());
        assert!(live.is_dirty());
    }

    #[test]
    fn detached_midi_import_rejects_a_result_after_an_edit() {
        let scratch = Scratch::new("detached-midi-stale");
        let midi = scratch.join("Part.mid");
        let mut source = Project::new("Part", 48_000.0);
        let track = source.add_instrument_track("MIDI", "fixture.instrument");
        let clip = source
            .add_midi_clip(track, "MIDI", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        source
            .midi_clip_mut(clip)
            .unwrap()
            .notes
            .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
        auris_io::write_midi_file(&midi, &source).unwrap();

        let mut live = session();
        let job = live.begin_midi_import(&midi).unwrap();
        live.add_default_instrument_track("Edit during import")
            .unwrap();
        let edited = live.project().clone();
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();

        assert!(live.continue_midi_import(result).is_none());
        assert_eq!(live.project(), &edited);
        assert!(live.can_undo());
    }

    #[test]
    fn midi_import_expires_discovery_handles_from_the_replaced_unsaved_document() {
        let mut live = session();
        let previous_sound_scope = live.sound_scope.clone();
        let result = MidiImportResult {
            owner: Arc::clone(&live.registry),
            revision: live.revision,
            project: Project::new("Imported", 48_000.0),
            report: MidiReport {
                tracks: 0,
                notes: 0,
                length: Ticks::ZERO,
            },
        };

        assert!(live.continue_midi_import(result).is_some());
        assert_ne!(live.sound_scope, previous_sound_scope);
        assert!(live.path().is_none());
    }

    #[test]
    fn detached_midi_export_only_replaces_bytes_after_current_uncancelled_continuation() {
        let scratch = Scratch::new("detached-midi-export");
        let cancelled_path = scratch.join("cancelled.mid");
        let stale_path = scratch.join("stale.mid");
        let current_path = scratch.join("current.mid");
        for path in [&cancelled_path, &stale_path, &current_path] {
            std::fs::write(path, b"previous MIDI bytes").unwrap();
        }
        let mut live = session();
        let track = live.add_default_instrument_track("MIDI").unwrap();
        let clip = live
            .add_midi_clip(track, "MIDI", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        live.add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        assert!(
            live.begin_midi_export(&cancelled_path)
                .run(&AtomicBool::new(true))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read(&cancelled_path).unwrap(),
            b"previous MIDI bytes"
        );

        let before_staging = directory_entries(stale_path.parent().unwrap());
        let job = live.begin_midi_export(&stale_path);
        live.add_default_instrument_track("Edit during export")
            .unwrap();
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(&stale_path).unwrap(),
            b"previous MIDI bytes",
            "the worker must not publish before session revalidation"
        );
        assert!(
            live.continue_midi_export(result, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&stale_path).unwrap(), b"previous MIDI bytes");
        assert_eq!(
            directory_entries(stale_path.parent().unwrap()),
            before_staging
        );

        let cancelled_after_staging = AtomicBool::new(false);
        let result = live
            .begin_midi_export(&current_path)
            .run(&cancelled_after_staging)
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(&current_path).unwrap(),
            b"previous MIDI bytes"
        );
        cancelled_after_staging.store(true, Ordering::Relaxed);
        assert!(
            live.continue_midi_export(result, &cancelled_after_staging)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read(&current_path).unwrap(),
            b"previous MIDI bytes"
        );

        let result = live
            .begin_midi_export(&current_path)
            .run(&AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(&current_path).unwrap(),
            b"previous MIDI bytes"
        );
        assert_eq!(
            live.continue_midi_export(result, &AtomicBool::new(false))
                .unwrap(),
            Some((current_path.clone(), 1))
        );
        assert_eq!(&std::fs::read(&current_path).unwrap()[..4], b"MThd");
        assert_eq!(
            directory_entries(current_path.parent().unwrap()),
            before_staging
        );
    }

    #[test]
    fn failed_midi_export_preserves_existing_bytes_without_scratch_residue() {
        let scratch = Scratch::new("failed-midi-export");
        let path = scratch.join("existing.mid");
        std::fs::write(&path, b"previous MIDI bytes").unwrap();
        let before = directory_entries(path.parent().unwrap());
        let mut live = session();
        let track = live.add_default_instrument_track("MIDI").unwrap();
        let clip = live
            .add_midi_clip(track, "Too far away", Ticks(0x1000_0000), Ticks::QUARTER)
            .unwrap();
        live.add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        assert!(
            live.begin_midi_export(&path)
                .run(&AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"previous MIDI bytes");
        assert_eq!(directory_entries(path.parent().unwrap()), before);
    }

    #[test]
    fn detached_collect_reports_progress_and_only_rewrites_an_unchanged_document() {
        let scratch = Scratch::new("detached-collect");
        let loose = scratch.tone("loose.wav");
        let document = scratch.join("Song.auris");
        let mut live = session();
        live.import_audio(&loose, Ticks::ZERO).unwrap();
        live.save(&document).unwrap();
        let source = live.project().audio_sources.values().next().unwrap().id;
        assert!(!live.project().audio_sources[&source].path.is_inside());

        let job = live.begin_collect_assets().unwrap();
        let mut progress = Vec::new();
        let result = job
            .run(&AtomicBool::new(false), |fraction| progress.push(fraction))
            .unwrap();
        assert_eq!(progress.last(), Some(&1.0));
        live.add_default_instrument_track("Edit during copy")
            .unwrap();
        let edited = live.project().clone();
        assert!(live.continue_collect_assets(result).is_none());
        assert_eq!(live.project(), &edited);
        assert!(!live.project().audio_sources[&source].path.is_inside());

        let result = live
            .begin_collect_assets()
            .unwrap()
            .run(&AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(live.continue_collect_assets(result).unwrap().unwrap(), 1);
        assert!(live.project().audio_sources[&source].path.is_inside());
        assert!(live.is_dirty());
    }

    #[test]
    fn detached_collect_observes_cancellation_before_copying() {
        let scratch = Scratch::new("detached-collect-cancel");
        let loose = scratch.tone("loose.wav");
        let document = scratch.join("Song.auris");
        let mut live = session();
        live.import_audio(&loose, Ticks::ZERO).unwrap();
        live.save(&document).unwrap();
        let cancelled = AtomicBool::new(true);

        assert!(
            live.begin_collect_assets()
                .unwrap()
                .run(&cancelled, |_| {})
                .is_none()
        );
        assert!(!document.parent().unwrap().join(AUDIO_DIR).exists());
        assert!(
            live.project()
                .audio_sources
                .values()
                .all(|source| !source.path.is_inside())
        );
    }

    #[test]
    fn a_downloaded_font_reuses_its_samples_without_becoming_an_edit() {
        let scratch = Scratch::new("downloaded-font");
        let path = scratch.soundfont("GM.sf2");
        let loaded = read_soundfont(&path).unwrap();
        let samples = Arc::clone(&loaded.0);
        let mut session = session();
        let font = session.install_shipped_soundfont(&path, loaded);
        assert!(!session.is_dirty());
        assert!(!session.can_undo());
        assert_eq!(session.soundfont_preset_count(font), 1);
        assert_eq!(session.soundfont_presets(font)[0].name, "Test Piano");
        assert!(Arc::ptr_eq(&session.fonts.get(font).unwrap(), &samples));

        // A later document has a fresh id bank, but cannot reread the now-invalid file.
        std::fs::write(&path, b"not a font").unwrap();
        session.new_project();
        let font = session.adopt_font(&path).unwrap();
        assert!(Arc::ptr_eq(&session.fonts.get(font).unwrap(), &samples));
        assert!(Arc::ptr_eq(
            &session.font_cache.get(&path).unwrap().samples,
            &samples
        ));
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_download_finishing_after_undo_survives_both_branches_and_keeps_the_saved_state_clean() {
        let scratch = Scratch::new("download-after-undo");
        let path = scratch.soundfont("GM.sf2");
        let mut session = session();
        session.forget_history();
        let first = session.add_default_instrument_track("First").unwrap();
        let second = session.add_default_instrument_track("Second").unwrap();
        session.undo().unwrap();
        let font = session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert!(font.0 > second.0, "the redo branch already owns that id");
        assert!(session.is_dirty());
        assert!(session.can_redo());

        session.undo().unwrap();
        assert!(!session.is_dirty());
        assert!(session.project().track(first).is_none());
        assert!(session.project().soundfonts.contains_key(&font));
        assert!(session.soundfont_is_loaded(font));
        session.redo().unwrap();
        session.redo().unwrap();
        assert!(session.project().track(second).is_some());
        assert!(session.project().soundfonts.contains_key(&font));
        assert!(session.soundfont_is_loaded(font));
        assert!(session.is_dirty());
    }

    #[test]
    fn a_download_during_a_gesture_is_retained_when_the_gesture_is_reverted_or_unchanged() {
        let scratch = Scratch::new("download-during-gesture");
        let path = scratch.soundfont("GM.sf2");
        let mut session = session();
        let track = session.add_default_instrument_track("Original").unwrap();
        session.forget_history();
        session.begin_transaction(Edit::RenameTrack);
        session.rename_track(track, "Dragging").unwrap();
        let font = session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert!(session.is_dirty());
        assert!(session.revert_transaction());
        assert_eq!(session.project().track(track).unwrap().name, "Original");
        assert!(session.soundfont_is_loaded(font));
        assert!(session.project().soundfonts.contains_key(&font));
        assert!(!session.is_dirty());
        assert!(!session.can_undo());

        session.begin_transaction(Edit::RenameTrack);
        session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert!(!session.end_transaction());
        assert!(!session.is_dirty());
        assert!(!session.can_undo());
    }

    #[test]
    fn library_arrival_does_not_change_the_disk_conflict_baseline() {
        let scratch = Scratch::new("download-disk-baseline");
        let path = scratch.soundfont("GM.sf2");
        let document = scratch.join("Song.auris");
        let mut session = session();
        session.save(&document).unwrap();
        let disk = load_project(&document).unwrap();
        session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert_eq!(session.saved_project, disk);
        assert!(!session.is_dirty());
        session.add_default_instrument_track("Lead").unwrap();
        session.undo().unwrap();
        assert!(!session.is_dirty());
        session.save_in_place().unwrap();

        let mut outside = load_project(&document).unwrap();
        outside.name = "Someone else's edit".into();
        save_project(&document, &mut outside).unwrap();
        session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert!(matches!(
            session.save_in_place(),
            Err(SessionError::ExternalChanges(_))
        ));
    }

    #[test]
    fn a_download_recovers_a_missing_saved_font_under_its_original_id() {
        let scratch = Scratch::new("download-missing-reference");
        let path = scratch.soundfont("GM.sf2");
        let old_path = scratch.join("another-machine/GM.sf2");
        let document = scratch.join("Song.auris");
        let mut session = session();
        let original = session.project.add_soundfont(
            "Saved GM",
            AssetPath::external(&old_path),
            byte_size(&path),
        );
        session.save(&document).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(session.open(&document).unwrap(), vec![old_path]);
        assert!(!session.soundfont_is_loaded(original));
        std::fs::write(&path, bytes).unwrap();
        let loaded = read_soundfont(&path).unwrap();
        let samples = Arc::clone(&loaded.0);
        let installed = session.install_shipped_soundfont(&path, loaded);
        assert_eq!(installed, original);
        assert_eq!(session.soundfonts().count(), 1);
        assert_eq!(
            session.project().soundfonts[&original].path,
            AssetPath::external(&path)
        );
        assert!(Arc::ptr_eq(&session.fonts.get(original).unwrap(), &samples));
        assert!(!session.is_dirty());
        session.add_default_instrument_track("Lead").unwrap();
        session.undo().unwrap();
        assert!(!session.is_dirty());
        assert_eq!(session.soundfonts().count(), 1);
        session.save_in_place().unwrap();
    }

    #[test]
    fn a_download_does_not_replace_a_same_named_font_of_a_different_size() {
        let scratch = Scratch::new("download-wrong-reference");
        let path = scratch.soundfont("GM.sf2");
        let mut session = session();
        let missing = session.project.add_soundfont(
            "Another GM",
            AssetPath::external(scratch.join("elsewhere/GM.sf2")),
            byte_size(&path) + 1,
        );
        session.forget_history();
        let font = session.install_shipped_soundfont(&path, read_soundfont(&path).unwrap());
        assert_ne!(font, missing);
        assert_eq!(session.soundfonts().count(), 2);
        assert!(!session.soundfont_is_loaded(missing));
        assert!(session.soundfont_is_loaded(font));
    }

    #[test]
    fn historical_font_ids_cannot_overwrite_the_current_fonts_samples() {
        let scratch = Scratch::new("download-reused-font-id");
        let shipped = scratch.soundfont("GM.sf2");
        let other = scratch.soundfont("Other.sf2");
        let mut session = session();
        let old_id = session.project.add_soundfont(
            "Missing GM",
            AssetPath::external(scratch.join("elsewhere/GM.sf2")),
            byte_size(&shipped),
        );
        session
            .history
            .push(Edit::ImportSoundFont, &session.project);
        // Snapshots can come from different saved versions, whose independent allocators gave
        // this id to different files. The loaded bank can only hold one meaning at a time.
        let mut other_version = Project::new("Other version", 48_000.0);
        let other_id =
            other_version.add_soundfont("Other", AssetPath::external(&other), byte_size(&other));
        assert_eq!(old_id, other_id);
        session.replace_project(other_version);
        session.import_soundfont(&other).unwrap();
        let other_samples = session.fonts.get(other_id).unwrap();
        let loaded = read_soundfont(&shipped).unwrap();
        let shipped_samples = Arc::clone(&loaded.0);
        session.install_shipped_soundfont(&shipped, loaded);
        assert!(Arc::ptr_eq(
            &session.fonts.get(other_id).unwrap(),
            &other_samples
        ));

        // Neither restoration may touch the disk, even though the two paths share an id.
        std::fs::write(&shipped, b"unreadable now").unwrap();
        std::fs::write(&other, b"unreadable now").unwrap();
        session.undo().unwrap();
        assert!(Arc::ptr_eq(
            &session.fonts.get(old_id).unwrap(),
            &shipped_samples
        ));
        session.redo().unwrap();
        assert!(Arc::ptr_eq(
            &session.fonts.get(other_id).unwrap(),
            &other_samples
        ));
        session.new_project();
        assert!(session.font_cache.contains_key(&shipped));
        assert!(!session.font_cache.contains_key(&other));
    }

    #[test]
    fn a_collected_font_keeps_its_cached_samples_after_undo() {
        let scratch = Scratch::new("collected-font-undo");
        let path = scratch.soundfont("GM.sf2");
        let mut session = session();
        let font = session.import_soundfont(&path).unwrap();
        let samples = session.fonts.get(font).unwrap();
        session.save_as(&scratch.join("Song.auris")).unwrap();
        assert_eq!(session.collect_assets().unwrap(), 1);
        session.save_in_place().unwrap();
        assert_eq!(
            session.project().soundfonts[&font].path,
            AssetPath::inside("Audio/GM.sf2")
        );
        let collected = session.project().soundfonts[&font]
            .path
            .resolve(session.project_folder())
            .unwrap();
        std::fs::write(&collected, b"unreadable now").unwrap();
        session.add_default_instrument_track("Lead").unwrap();
        session.undo().unwrap();
        assert!(Arc::ptr_eq(&session.fonts.get(font).unwrap(), &samples));
        assert!(!session.is_dirty());
    }

    #[test]
    fn external_reload_still_marks_relocated_audio_dirty() {
        let scratch = Scratch::new("external-relocated-audio");
        let audio = scratch.tone("Tone.wav");
        let document = scratch.join("Song.auris");
        let mut session = session();
        session.import_audio(&audio, Ticks::ZERO).unwrap();
        session.save(&document).unwrap();
        let directory = scratch.join(AUDIO_DIR);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::rename(&audio, directory.join("Tone.wav")).unwrap();
        let mut outside = load_project(&document).unwrap();
        outside.name = "Outside edit".into();
        save_project(&document, &mut outside).unwrap();
        assert!(session.reload_external_changes().unwrap().is_empty());
        assert!(session.is_dirty());
        assert_eq!(
            session
                .project()
                .audio_sources
                .values()
                .next()
                .unwrap()
                .path,
            AssetPath::inside("Audio/Tone.wav")
        );
        session.save_in_place().unwrap();
    }

    #[test]
    fn external_edits_cannot_be_overwritten_and_acceptance_preserves_both_versions() {
        let scratch = Scratch::new("external-edit");
        let path = scratch.join("Song.auris");
        let mut window = session();
        window.save(&path).unwrap();
        let mut agent = session();
        agent.open(&path).unwrap();
        window.add_default_instrument_track("Local").unwrap();
        let local = window.project().clone();
        agent.add_default_instrument_track("Agent").unwrap();
        agent.save_in_place().unwrap();
        let disk = agent.project().clone();
        assert!(matches!(
            window.save_in_place(),
            Err(SessionError::ExternalChanges(_))
        ));
        assert_eq!(load_project(&path).unwrap(), disk);
        window.reload_external_changes().unwrap();
        assert_eq!(window.project(), &disk);
        assert_eq!(window.undo(), Some(crate::Edit::ExternalChanges));
        assert_eq!(window.project(), &local);
        assert!(window.is_dirty());
        assert_eq!(window.redo(), Some(crate::Edit::ExternalChanges));
        assert_eq!(window.project(), &disk);
        assert!(!window.is_dirty());
        window.undo();
        window.undo();
        assert!(
            !window
                .project()
                .tracks
                .iter()
                .any(|track| track.name == "Local")
        );
    }

    #[test]
    fn failed_reload_keeps_the_document_and_undo_history() {
        let scratch = Scratch::new("failed-external-edit");
        let path = scratch.join("Song.auris");
        let mut window = session();
        window.save(&path).unwrap();
        window.add_default_instrument_track("Local").unwrap();
        let before = window.project().clone();
        std::fs::write(&path, "not a project").unwrap();
        assert!(window.reload_external_changes().is_err());
        assert_eq!(window.project(), &before);
        assert!(window.can_undo());
    }

    #[test]
    fn audio_read_somewhere_else_lands_exactly_as_importing_it_would() {
        // The two halves together have to be the whole of the one call: a frontend that decodes on
        // a worker thread must not get a quieter version of an import.
        let scratch = Scratch::new("placed");
        let file = scratch.tone("kick.wav");

        let mut whole = session();
        let expected = whole.import_audio(&file, Ticks::ZERO).expect("imports");

        let mut split = session();
        let buffer = decode_audio(&file, split.project().sample_rate).expect("decodes");
        let clip = split
            .place_audio(&file, buffer, Ticks::ZERO)
            .expect("places");

        assert_eq!(clip, expected, "the same clip id, made the same way");
        assert_eq!(split.project(), whole.project());
        assert!(split.can_undo(), "an import is one undo step either way");
    }

    #[test]
    fn audio_decoded_against_a_rate_the_document_no_longer_has_is_decoded_again() {
        // What the split makes possible: the document can move while the file is being read. A
        // buffer at yesterday's rate would play at the wrong pitch and the wrong length, so it is
        // the buffer that is thrown away rather than the import that is refused.
        let scratch = Scratch::new("restale");
        let file = scratch.tone("kick.wav");

        let mut session = session();
        let stale = decode_audio(&file, session.project().sample_rate / 2.0).expect("decodes");
        assert_ne!(stale.sample_rate(), session.project().sample_rate);

        session
            .place_audio(&file, stale, Ticks::ZERO)
            .expect("places");
        let source = session
            .project()
            .audio_sources
            .values()
            .next()
            .expect("one");
        assert_eq!(source.sample_rate, session.project().sample_rate);
    }

    #[test]
    fn importing_a_soundfont_that_is_not_there_changes_nothing() {
        // The picker hands over whatever the user chose, and a file can be gone by the time it
        // is opened. Failing has to leave the document exactly as it was.
        let mut session = session();
        session.forget_history();

        let refused = session.import_soundfont(Path::new("no-such-soundfont.sf2"));
        assert!(refused.is_err());
        assert_eq!(session.soundfonts().count(), 0);
        assert!(!session.can_undo(), "a failed import left a step behind");
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_font_the_project_names_but_has_not_loaded_is_reported_as_such() {
        // What a library panel needs in order to say "this file has moved" rather than showing
        // an empty list of sounds and leaving the user to guess.
        let mut session = session();
        let font = named_font(&mut session, "Orchestra");
        assert_eq!(session.soundfonts().count(), 1);
        assert!(!session.soundfont_is_loaded(font));
        assert!(session.soundfont_presets(font).is_empty());
    }

    #[test]
    fn saving_over_another_project_is_refused_until_it_is_agreed_to() {
        // The system save dialog offers to replace the *name* that was typed. A project is
        // written one folder deeper than that, so the dialog never saw the document that would
        // actually be destroyed and asked nothing.
        let scratch = Scratch::new("would-replace");
        let mut first = session();
        first.add_default_instrument_track("Old").unwrap();
        let existing = first
            .save_as(&scratch.join("Ballad.auris"))
            .expect("saves")
            .document;
        assert!(existing.exists());

        let mut second = session();
        second.add_default_instrument_track("New").unwrap();
        let refused = second.save_as(&scratch.join("Ballad.auris")).unwrap_err();
        assert!(
            matches!(&refused, SessionError::WouldReplace(path) if *path == existing),
            "the error names the document that would go, not the name that was typed",
        );
        assert!(
            second.is_dirty(),
            "nothing was written, so nothing is saved"
        );
        assert_eq!(
            load_project(&existing).unwrap().tracks[0].name,
            "Old",
            "refusing replacement must leave the existing document untouched"
        );

        // And with the replacement agreed to it goes ahead.
        second
            .save_as_replacing(&scratch.join("Ballad.auris"))
            .expect("replaces");
        let reopened = {
            let mut session = session();
            session.open(&existing).expect("opens");
            session.project().tracks[0].name.clone()
        };
        assert_eq!(reopened, "New");
    }

    #[test]
    fn simultaneous_first_saves_cannot_both_claim_the_same_project() {
        #[derive(Debug)]
        enum Outcome {
            Saved(String),
            Refused(String),
        }

        let scratch = Scratch::new("simultaneous-save-as");
        let chosen = scratch.join("Ballad.auris");
        let document = document_in_folder(&chosen);
        std::fs::create_dir_all(document.parent().unwrap()).unwrap();
        let blocker = project_write_lock(&document).expect("the test holds the project lock");
        let start = Arc::new(Barrier::new(3));

        let handles: Vec<_> = ["First", "Second"]
            .into_iter()
            .map(|name| {
                let chosen = chosen.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    let mut session = session();
                    session.add_default_instrument_track(name).unwrap();
                    start.wait();
                    match session.save_as(&chosen) {
                        Ok(_) => Outcome::Saved(name.to_string()),
                        Err(SessionError::WouldReplace(_)) => Outcome::Refused(name.to_string()),
                        Err(error) => panic!("unexpected save error: {error}"),
                    }
                })
            })
            .collect();

        start.wait();
        // Both callers can finish the old check-before-lock path while this lock is held. The
        // assertion remains scheduling-independent after the check moves under the lock.
        std::thread::sleep(Duration::from_millis(100));
        drop(blocker);
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("save thread finishes"))
            .collect();

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Outcome::Saved(_)))
                .count(),
            1,
            "exactly one caller may create a previously absent project: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Outcome::Refused(_)))
                .count(),
            1,
            "the losing caller must be told that replacement needs consent: {outcomes:?}"
        );
        let winner = outcomes
            .iter()
            .find_map(|outcome| match outcome {
                Outcome::Saved(name) => Some(name),
                Outcome::Refused(_) => None,
            })
            .unwrap();
        let refused = outcomes
            .iter()
            .find_map(|outcome| match outcome {
                Outcome::Refused(name) => Some(name),
                Outcome::Saved(_) => None,
            })
            .unwrap();
        let stored = load_project(&document).unwrap();
        assert!(stored.tracks.iter().any(|track| &track.name == winner));
        assert!(!stored.tracks.iter().any(|track| &track.name == refused));
    }

    #[test]
    fn a_failed_save_as_rolls_back_to_the_previous_location() {
        let scratch = Scratch::new("failed-save-as-dirty");
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.save_as(&scratch.join("Original.auris")).unwrap();
        assert!(!session.is_dirty());

        let chosen = scratch.join("Blocked.auris");
        let document = document_in_folder(&chosen);
        std::fs::create_dir_all(&document).unwrap();

        assert!(session.save_as_replacing(&chosen).is_err());
        let original = document_in_folder(&scratch.join("Original.auris"));
        assert_eq!(session.path(), Some(original.as_path()));
        assert!(
            !session.is_dirty(),
            "a failed save must not mutate live state"
        );
        std::fs::remove_dir(&document).unwrap();
        session
            .save_as_replacing(&chosen)
            .expect("retry at the requested destination");
        assert_eq!(session.path(), Some(document.as_path()));
        assert!(!session.is_dirty());
    }

    #[test]
    fn saving_back_over_this_project_is_not_a_replacement() {
        let scratch = Scratch::new("save-over-itself");
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        let document = session
            .save_as(&scratch.join("Ballad.auris"))
            .expect("saves")
            .document;

        session.add_default_instrument_track("Bass").unwrap();
        let again = session.save_as(&document).expect("saves over itself");
        assert_eq!(again.document, document);
    }

    #[test]
    fn a_project_round_trips_through_a_file() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.set_bpm(96.0);

        let path = std::env::temp_dir().join("auris-session-round-trip.auris");
        session.save(&path).unwrap();
        assert!(!session.is_dirty());
        let saved = session.project().clone();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&path).unwrap();
        assert!(missing.is_empty());
        assert_eq!(reopened.project(), &saved);
        assert!(!reopened.can_undo(), "opening must not be undoable");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn opening_another_document_clears_arms_and_monitors_even_when_ids_collide() {
        let scratch = Scratch::new("open-clears-device-bindings");
        let mut other = session();
        let other_track = other.add_audio_track("Other");
        let path = other
            .save_as(&scratch.join("Other.auris"))
            .unwrap()
            .document;

        let mut current = session();
        let current_track = current.add_audio_track("Current");
        assert_eq!(
            current_track, other_track,
            "the reproduction needs reused ids"
        );
        current.arm_track(current_track, None).unwrap();
        current.set_track_monitoring(current_track, true).unwrap();

        current.open(&path).unwrap();

        assert!(current.armed_tracks().is_empty());
        assert!(current.monitored_tracks().is_empty());
    }

    #[test]
    fn saving_under_a_new_name_gathers_the_song_into_one_folder() {
        let scratch = Scratch::new("gather");
        let loose = scratch.tone("kick.wav");

        let mut session = session();
        session.import_audio(&loose, Ticks::ZERO).expect("imports");
        let document = session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves")
            .document;

        assert_eq!(document, scratch.join("MySong").join("MySong.auris"));
        assert!(
            scratch
                .join("MySong")
                .join("Audio")
                .join("kick.wav")
                .is_file(),
            "the audio has to travel with the document"
        );
        let source = session.project().audio_sources.values().next().unwrap();
        assert_eq!(
            source.path,
            AssetPath::inside(Path::new("Audio").join("kick.wav")),
            "and the document has to refer to its own copy"
        );
    }

    #[test]
    fn a_project_folder_that_has_been_moved_still_opens() {
        // The whole reason for relative references. Nothing here touches the document: the folder
        // is renamed underneath it, which is what a person dragging it somewhere else does.
        let scratch = Scratch::new("moved");
        let loose = scratch.tone("kick.wav");

        let mut session = session();
        session.import_audio(&loose, Ticks::ZERO).expect("imports");
        assert_eq!(
            session
                .save_as(&scratch.join("Before.auris"))
                .expect("saves")
                .document,
            scratch.join("Before").join("Before.auris")
        );
        drop(session);
        std::fs::remove_file(&loose).expect("the file it was imported from goes away too");

        let moved = scratch.join("After");
        std::fs::rename(scratch.join("Before"), &moved).expect("the folder moves");

        let mut reopened = self::tests::session();
        let missing = reopened
            .open(&moved.join("Before.auris"))
            .expect("the moved project opens");
        assert!(missing.is_empty(), "nothing should be missing: {missing:?}");
        assert_eq!(reopened.project().audio_sources.len(), 1);
    }

    #[test]
    fn a_copy_that_fails_during_save_as_leaves_a_reference_that_still_opens() {
        // The document belongs to the new folder before a single file has been copied there, so an
        // `Inside` reference left untouched by a failed copy stops naming the file it was written
        // for and starts naming one that was never made. Silent track, and no way back: the repair
        // command only looks at references that are *not* inside.
        let scratch = Scratch::new("save-as-copy-fails");
        let mut session = session();
        session
            .save_as(&scratch.join("First.auris"))
            .expect("the first save works");
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .expect("imports");
        let owned = scratch.join("First").join(AUDIO_DIR).join("kick.wav");
        assert!(owned.is_file(), "the first save owns its copy");

        // A file where the new folder's `Audio/` directory needs to go: `copy_into` starts with
        // `create_dir_all`, so every copy into this folder fails while the originals stay put.
        std::fs::create_dir_all(scratch.join("Second")).unwrap();
        std::fs::write(scratch.join("Second").join(AUDIO_DIR), b"in the way").unwrap();

        let report = session
            .save_as(&scratch.join("Second.auris"))
            .expect("the document still saves — a missing asset is reported, never fatal");
        assert_eq!(report.uncollected, vec![owned.clone()]);

        let source = session.project().audio_sources.values().next().unwrap();
        assert!(
            !source.path.is_inside(),
            "an inside reference here would be read against the folder the copy never reached"
        );
        assert_eq!(
            source.path.resolve(session.project_folder()),
            Some(owned),
            "the file it was resolved from is still there, so that is what to name"
        );

        // The whole point of the fallback: `SaveReport` promises the project opens on this machine.
        let mut reopened = self::tests::session();
        let missing = reopened
            .open(&report.document)
            .expect("the saved project opens");
        assert!(missing.is_empty(), "nothing should be missing: {missing:?}");
        assert_eq!(reopened.project().audio_sources.len(), 1);
    }

    #[test]
    fn audio_imported_into_a_saved_project_is_copied_in_at_once() {
        let scratch = Scratch::new("import-after-save");
        let mut session = session();
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");

        let loose = scratch.tone("snare.wav");
        session.import_audio(&loose, Ticks::ZERO).expect("imports");

        assert!(
            scratch
                .join("MySong")
                .join("Audio")
                .join("snare.wav")
                .is_file()
        );
        let source = session.project().audio_sources.values().next().unwrap();
        assert!(source.path.is_inside());
    }

    #[test]
    fn a_soundfont_is_referred_to_where_it_lies() {
        // The policy that pays for itself: a font is a library, and twenty projects using one
        // must not mean twenty copies of it.
        let scratch = Scratch::new("font-external");
        let font = scratch.join("GM.sf2");
        std::fs::write(&font, b"not a real font").unwrap();

        let mut session = session();
        // The file is not a SoundFont, so the import fails — but the document must not have
        // gained a reference to it either way.
        assert!(session.import_soundfont(&font).is_err());

        session
            .project
            .add_soundfont("GM", AssetPath::external(&font), auris_io::byte_size(&font));
        let stored = session.project().soundfonts.values().next().unwrap();
        assert!(!stored.path.is_inside());
        assert_eq!(stored.byte_size, 15);
    }

    #[test]
    fn collecting_ignores_files_that_never_decoded_as_assets() {
        let scratch = Scratch::new("collect-untrusted");
        let secret = scratch.join("secret.txt");
        std::fs::write(&secret, b"not audio and not a SoundFont").unwrap();

        let mut session = session();
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");
        session
            .project
            .add_audio_source("Private", AssetPath::external(&secret), 1, 48_000.0, 1);
        session.project.add_soundfont(
            "Private",
            AssetPath::external(&secret),
            auris_io::byte_size(&secret),
        );

        assert_eq!(session.collect_assets().expect("skips undecoded files"), 0);
        assert!(
            !scratch
                .join("MySong")
                .join("Audio")
                .join("secret.txt")
                .exists(),
            "a document reference alone must not turn an arbitrary local file into an asset"
        );
        assert!(
            session
                .project()
                .audio_sources
                .values()
                .all(|source| !source.path.is_inside())
        );
        assert!(
            session
                .project()
                .soundfonts
                .values()
                .all(|font| !font.path.is_inside())
        );
    }

    #[test]
    fn saving_under_a_new_name_takes_a_font_the_project_already_owns_with_it() {
        // A collected font lives in the *old* folder. Carrying its reference across unchanged
        // reported success, went on sounding here from the samples already in memory, and opened
        // silent on the machine the copy was made for.
        let scratch = Scratch::new("font-travels");
        let library = scratch.join("GM.sf2");
        std::fs::write(&library, b"stand-in for a very large font").unwrap();

        let mut session = session();
        session
            .save_as(&scratch.join("First.auris"))
            .expect("saves");
        let font = session.project.add_soundfont(
            "GM",
            AssetPath::external(&library),
            auris_io::byte_size(&library),
        );
        // This test is about carrying an already-owned file across Save As. The low-level copy
        // establishes that state without pretending the stand-in bytes decoded as a SoundFont.
        session
            .collect_font(font, &library)
            .expect("collects fixture");
        // The library it came from goes away, so nothing below can be reading the original.
        std::fs::remove_file(&library).unwrap();

        let report = session
            .save_as(&scratch.join("Second.auris"))
            .expect("saves again");
        assert!(
            report.uncollected.is_empty(),
            "nothing should have been left behind: {:?}",
            report.uncollected
        );

        let second = scratch.join("Second");
        assert!(
            second.join(AUDIO_DIR).join("GM.sf2").is_file(),
            "a font the project owns has to travel with it"
        );
        assert!(
            scratch
                .join("First")
                .join(AUDIO_DIR)
                .join("GM.sf2")
                .is_file(),
            "and Save As copies rather than moves, so the project saved from still has its own"
        );
        let stored = session.project().soundfonts.values().next().unwrap();
        assert_eq!(
            stored.path.resolve(session.project_folder()),
            Some(second.join(AUDIO_DIR).join("GM.sf2")),
            "the stored reference has to resolve to the copy in the new folder"
        );
    }

    #[test]
    fn saving_under_a_new_name_leaves_a_font_in_its_library_alone() {
        // The policy Save As must not quietly change: a font is shared by every project that uses
        // it, and `collect_assets` is the opt-in that pays for a copy.
        let scratch = Scratch::new("font-stays");
        let library = scratch.join("GM.sf2");
        std::fs::write(&library, b"stand-in for a very large font").unwrap();

        let mut session = session();
        session.project.add_soundfont(
            "GM",
            AssetPath::external(&library),
            auris_io::byte_size(&library),
        );
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");

        assert!(
            !scratch
                .join("MySong")
                .join(AUDIO_DIR)
                .join("GM.sf2")
                .exists(),
            "hundreds of megabytes per save is what this policy exists to avoid"
        );
        assert_eq!(
            session.project().soundfonts.values().next().unwrap().path,
            AssetPath::external(&library)
        );
    }

    #[test]
    fn collecting_needs_somewhere_to_collect_into() {
        let mut session = session();
        assert!(matches!(
            session.collect_assets(),
            Err(SessionError::NoPath)
        ));
    }

    #[test]
    fn new_project_clears_the_history_and_the_path() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.new_project();
        assert!(!session.can_undo());
        assert!(session.path().is_none());
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn the_door_knows_whose_save_it_is_reading() {
        let scratch = Scratch::new("saved-by");
        let mut session = session();
        // Never saved: there is no save to have come from anywhere.
        assert_eq!(session.saved_by_another_build(), None);

        let path = scratch.join("mine.auris");
        session.save(&path).unwrap();
        let mut reopened = crate::session::fixtures::session();
        reopened.open(&path).unwrap();
        // This build saved it, so the door has nothing to say.
        assert_eq!(reopened.saved_by_another_build(), None);

        // The same file with another build's name on it gets the note, and the note carries
        // the name.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(env!("CARGO_PKG_VERSION"), "0.1.0");
        std::fs::write(&path, text).unwrap();
        reopened.open(&path).unwrap();
        assert_eq!(reopened.saved_by_another_build(), Some("0.1.0"));

        // A file from before the record existed is an older build by definition — one that
        // left no name, which is what the empty answer says.
        let unsigned: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|line| !line.contains("saved_by"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, unsigned).unwrap();
        reopened.open(&path).unwrap();
        assert_eq!(reopened.saved_by_another_build(), Some(""));
    }
}
