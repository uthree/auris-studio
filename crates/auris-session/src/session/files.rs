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

use super::{MidiReport, SaveReport, Session};

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

impl Session {
    /// Replaces the document with an empty project holding one instrument track.
    pub fn new_project(&mut self) {
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
        let imported = auris_io::read_midi_file(path)?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        let fallback = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        let drums = match self.registry.has_instrument(DRUM_INSTRUMENT) {
            true => DRUM_INSTRUMENT.to_string(),
            false => fallback.clone(),
        };

        let mut project = Project::new(name, self.project.sample_rate);
        project.tempo_map = imported.tempo_map.clone();
        project.signatures = imported.signatures.clone();
        let mut report = MidiReport {
            tracks: 0,
            notes: 0,
            length: imported.end(),
        };

        for track in &imported.tracks {
            let instrument = match track.channel {
                DRUM_CHANNEL => drums.clone(),
                _ => fallback.clone(),
            };
            let track_id = project.add_instrument_track(&track.name, instrument);
            // One clip per track, spanning the material rather than the song: a part that does not
            // start until bar forty gets a clip at bar forty, not forty bars of empty clip with
            // its notes at the far end.
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
                // Rebased the same way the notes are, and cut to the clip: a curve written before
                // the first note or after the last has nothing here to shape.
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
                // The file said where the notes are; nothing should grow the clip past them on the
                // next edit and quietly change what it holds.
                clip.length_is_explicit = true;
                report.notes += clip.notes.len();
            }
            report.tracks += 1;
        }

        self.history.clear();
        self.path = None;
        self.clear_sources();
        self.replace_project(project);
        self.install_shipped_fonts();
        // Dirty from the first frame: nothing on disk holds this document, and the `.mid` it came
        // from cannot hold it either.
        self.dirty = true;
        Ok(report)
    }

    /// Writes the open document's instrument tracks as a Standard MIDI File.
    ///
    /// Returns how many notes were written. What a `.mid` has nowhere to put — audio tracks, the
    /// mixer, which instrument each track plays, the automation — is left behind; see
    /// [`auris_io::midi`] for the whole list.
    pub fn export_midi(&self, path: &Path) -> Result<usize, SessionError> {
        Ok(auris_io::write_midi_file(path, &self.project)?)
    }

    /// The folder holding the current document, which relative asset paths resolve against.
    ///
    /// `None` for a project that has never been saved — and one of those has collected nothing,
    /// so every asset it names is still external and resolves without help.
    pub fn project_folder(&self) -> Option<&Path> {
        self.path.as_deref().and_then(auris_io::project_folder)
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

    /// Writes the document at exactly `path`, without moving or collecting anything.
    ///
    /// The project folder becomes the directory holding `path`, so a caller choosing a fresh
    /// location wants [`Self::save_as`] instead — this one would leave the audio behind.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        self.collect_hosted_state();
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
        let document = document_in_folder(chosen);
        // The system save dialog offered to replace whatever is at `chosen`. It is not what gets
        // written: a project goes into a folder named after the file, so choosing `Songs/Ballad`
        // when `Songs/Ballad/Ballad.auris` already exists looked to the dialog like a name
        // nothing was using, and destroyed last week's song without a word. Saving back over
        // *this* project is not a replacement and is allowed to proceed.
        if document.exists() && self.path.as_deref() != Some(document.as_path()) {
            return Err(SessionError::WouldReplace(document));
        }
        self.save_as_replacing(chosen)
    }

    /// [`Self::save_as`] with the replacement already agreed to.
    ///
    /// For a host that has shown the user which project is about to be overwritten and been told
    /// to go ahead. Nothing else differs.
    pub fn save_as_replacing(&mut self, chosen: &Path) -> Result<SaveReport, SessionError> {
        let document = document_in_folder(chosen);
        let folder = auris_io::project_folder(&document)
            .ok_or(SessionError::NoPath)?
            .to_path_buf();
        std::fs::create_dir_all(&folder).map_err(|source| IoError::Filesystem {
            path: folder.clone(),
            source,
        })?;
        let _lock = project_write_lock(&document)?;
        if document.is_file() {
            let previous_path = self.path.replace(document.clone());
            let preserved = load_project(&document)
                .map_err(SessionError::from)
                .and_then(|project| self.preserve_document(&project));
            self.path = previous_path;
            preserved?;
        }

        // Resolve before the document moves: an `Inside` reference read against the new folder
        // would point at a file that has not been copied there yet.
        let audio: Vec<(SourceId, Option<PathBuf>)> = self
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
        let fonts: Vec<(SoundFontId, Option<PathBuf>)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| font.path.is_inside())
            .map(|font| (font.id, font.path.resolve(self.project_folder())))
            .collect();

        // From here the document belongs to the new folder even if the write below fails: the
        // files land there, and their references are read against wherever `self.path` says the
        // document is. Leaving it pointing at the old folder is what would be inconsistent.
        self.path = Some(document.clone());
        self.dirty = true;
        if !document.is_file() {
            // A failed first write may be retried once its destination becomes writable.
            self.disk_stamp = None;
            self.disk_fingerprint = None;
        }
        // A copy that fails has to have its reference pointed back *outside*, not left alone. The
        // document belongs to the new folder from the line above, so an `Inside` reference is now
        // read against a folder the copy never reached: the track opens silent, and nothing can
        // repair it afterwards, because [`Self::collect_assets`] only looks at references that are
        // not already inside and would skip this one on every retry. The path it was resolved from
        // is a file that still exists, so naming it absolutely is what keeps
        // [`SaveReport::uncollected`]'s promise that the project opens on this machine.
        let mut uncollected = Vec::new();
        for (id, from) in audio {
            let Some(from) = from else { continue };
            if let Err(error) = self.collect_source(id, &from) {
                log::warn!("could not collect {}: {error}", from.display());
                if let Some(source) = self.project.audio_sources.get_mut(&id) {
                    source.path = AssetPath::external(&from);
                }
                uncollected.push(from);
            }
        }
        for (id, from) in fonts {
            let Some(from) = from else { continue };
            if let Err(error) = self.collect_font(id, &from) {
                log::warn!("could not collect {}: {error}", from.display());
                if let Some(font) = self.project.soundfonts.get_mut(&id) {
                    font.path = AssetPath::external(&from);
                }
                uncollected.push(from);
            }
        }

        self.collect_hosted_state();
        save_project(&document, &mut self.project)?;
        self.dirty = false;
        self.mark_saved();
        Ok(SaveReport {
            document,
            uncollected,
        })
    }

    /// Saves to the path the project was last saved to or opened from.
    pub fn save_in_place(&mut self) -> Result<(), SessionError> {
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        let _lock = project_write_lock(&path)?;
        if (path.exists() || self.disk_fingerprint.is_some())
            && load_project(&path)? != self.saved_project
        {
            return Err(SessionError::ExternalChanges(path));
        }
        self.save(&path)
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
        let missing = self.replace_external_project(project);
        self.mark_saved();
        self.saved_project = saved;
        self.dirty = self.project != self.saved_project;
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
        if self.project_folder().is_none() {
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
        let has_folder = self.project_folder().is_some();
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
    /// Nothing to install is the ordinary answer on a build nobody has run
    /// `tools/fetch-soundfonts.sh` for, and the application runs on its own instruments.
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
    /// The samples are cached by path in [`Self::shipped`], so the second call — after a
    /// **File → New**, which empties the id-keyed bank — costs a map lookup rather than two
    /// hundred megabytes of file.
    fn adopt_font(&mut self, path: &Path) -> Option<SoundFontId> {
        let font = self.shipped_font_data(path)?;
        let id = match self.project.soundfont_at(self.project_folder(), path) {
            Some(existing) => existing,
            None => {
                let name = font_name(&font, path);
                self.project
                    .add_soundfont(name, AssetPath::external(path), byte_size(path))
            }
        };
        self.fonts.insert(id, font);
        Some(id)
    }

    /// A shipped font's samples, read from the file the first time and cached after that.
    fn shipped_font_data(&mut self, path: &Path) -> Option<Arc<SoundFont>> {
        if let Some(font) = self.shipped.get(path) {
            return Some(Arc::clone(font));
        }
        let font = load_soundfont(path)
            .inspect_err(|error| log::warn!("{}: {error}", path.display()))
            .ok()?;
        self.shipped.insert(path.to_path_buf(), Arc::clone(&font));
        Some(font)
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
    use super::*;
    use crate::session::fixtures::{Scratch, named_font, session};
    use auris_io::AUDIO_DIR;

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
    fn a_failed_save_as_keeps_the_new_location_dirty() {
        let scratch = Scratch::new("failed-save-as-dirty");
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.save_as(&scratch.join("Original.auris")).unwrap();
        assert!(!session.is_dirty());

        let chosen = scratch.join("Blocked.auris");
        let document = document_in_folder(&chosen);
        std::fs::create_dir_all(&document).unwrap();

        assert!(session.save_as_replacing(&chosen).is_err());
        assert_eq!(session.path(), Some(document.as_path()));
        assert!(session.is_dirty(), "autosave must retry the failed write");
        std::fs::remove_dir(&document).unwrap();
        session
            .save_in_place()
            .expect("retry at the new destination");
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
