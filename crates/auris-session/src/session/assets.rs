//! How a saved document finds the files it only names.
//!
//! A project holds [`AssetPath`]s rather than paths, and the whole of this file is the arithmetic
//! that turns one back into a file on this machine: reading everything the document names,
//! searching for what has moved, confirming a candidate by size, copying an asset into the
//! project folder, and writing back into the document whatever the search turned up.
//!
//! It is separate from `files` because the two answer to different people. A command in `files`
//! is something the user asked for and can be refused; nothing here is asked for and nothing here
//! is fatal — a font that cannot be found costs one track its sound and the project still opens.
//! See [`crate::guide::documents`] for why the search runs in two passes and why what it finds is
//! written back.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::{AssetPath, AudioBuffer, AudioSourceBank, SoundFontId, SourceId};
use auris_gpu::compute_peaks;
use auris_io::{AUDIO_DIR, byte_size, copy_into, find_named, import_audio_file, load_soundfont};

use crate::error::SessionError;
use crate::render::source_at_rate;

use super::Session;

/// How many samples one waveform bucket covers.
///
/// At 256 a five-minute stereo file's peak data stays under a megabyte while still resolving
/// individual drum hits at normal zoom levels.
const WAVEFORM_BUCKET: u32 = 256;

impl Session {
    /// Reads every file the document names, reporting the references nothing could be found for.
    ///
    /// Two passes, because the second needs what the first learned. Anything whose stored
    /// reference is still true is read straight away; only then is there a set of directories
    /// that assets are demonstrably living in, which is where the ones that moved are looked for.
    pub(super) fn reload_assets(&mut self) -> Vec<PathBuf> {
        let rate = self.project.sample_rate;
        let folder = self.project_folder().map(Path::to_path_buf);

        let sources: Vec<(SourceId, AssetPath, u64)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.clone(), source.byte_size))
            .collect();
        let fonts: Vec<(SoundFontId, AssetPath, u64)> = self
            .project
            .soundfonts
            .values()
            .map(|font| (font.id, font.path.clone(), font.byte_size))
            .collect();

        let mut search = self.search_path();
        let mut missing = Vec::new();

        for (id, stored, size) in sources {
            let Some(found) = locate(&stored, folder.as_deref(), &search, size) else {
                log::warn!("no audio file for {stored}");
                missing.push(stored.as_stored().to_path_buf());
                continue;
            };
            match import_audio_file(&found, rate) {
                Ok(buffer) => {
                    self.relocate_source(id, &stored, &found);
                    remember_directory(&mut search, &found);
                    self.install_source(id, Arc::new(buffer));
                }
                Err(error) => {
                    log::warn!("could not reload {}: {error}", found.display());
                    missing.push(stored.as_stored().to_path_buf());
                }
            }
        }

        for (id, stored, size) in fonts {
            let Some(found) = locate(&stored, folder.as_deref(), &search, size) else {
                log::warn!("no SoundFont file for {stored}");
                missing.push(stored.as_stored().to_path_buf());
                continue;
            };
            match load_soundfont(&found) {
                Ok(font) => {
                    self.relocate_font(id, &stored, &found);
                    remember_directory(&mut search, &found);
                    self.fonts.insert(id, font);
                }
                Err(error) => {
                    log::warn!("could not reload {}: {error}", found.display());
                    missing.push(stored.as_stored().to_path_buf());
                }
            }
        }

        missing
    }

    /// Directories to look in for a file whose stored path has stopped being true.
    ///
    /// The project folder and its audio directory, which is where a file that travelled with the
    /// project will be. Callers add the directories that assets actually turn up in as they go,
    /// so a document naming twenty fonts in one folder finds all twenty once it has found one.
    ///
    /// Then the shipped library, because a project that names the SoundFont this application came
    /// with names it at the path it had on the machine it was saved on. Every installation has
    /// that file, somewhere of its own — so the one reference most likely to be broken by sending
    /// a project to somebody else is also the one that always has an answer.
    fn search_path(&self) -> Vec<PathBuf> {
        let mut roots = match self.project_folder() {
            Some(folder) => vec![folder.join(AUDIO_DIR), folder.to_path_buf()],
            None => Vec::new(),
        };
        roots.extend(crate::library::library_roots());
        roots
    }

    /// Looks again for the fonts the document could not find, now that `directory` is known to
    /// hold at least one of them.
    pub(super) fn recover_fonts_from(&mut self, directory: &Path) {
        let lost: Vec<(SoundFontId, AssetPath, u64)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| !self.fonts.contains(font.id))
            .map(|font| (font.id, font.path.clone(), font.byte_size))
            .collect();

        let search = [directory.to_path_buf()];
        for (id, stored, size) in lost {
            let Some(name) = stored.file_name() else {
                continue;
            };
            let Some(found) = find_named(name, &search, size) else {
                continue;
            };
            match load_soundfont(&found) {
                Ok(font) => {
                    log::info!("found {} again at {}", stored, found.display());
                    self.relocate_font(id, &stored, &found);
                    self.fonts.insert(id, font);
                }
                Err(error) => log::warn!("could not read {}: {error}", found.display()),
            }
        }
    }

    /// Copies one audio file into the project folder and points the document at the copy.
    pub(super) fn collect_source(&mut self, id: SourceId, from: &Path) -> Result<(), SessionError> {
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;
        let name = copy_into(from, &folder.join(AUDIO_DIR))?;
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.path = AssetPath::inside(Path::new(AUDIO_DIR).join(name));
        }
        // `copy_into` either copied these bytes or found a file already holding them, so the size
        // of the source is the size of the copy the document now names.
        self.record_source_size(id, from);
        Ok(())
    }

    /// Copies one SoundFont into the project folder and points the document at the copy.
    ///
    /// Whether a font *should* be copied is policy and belongs to the callers — a font is a
    /// library shared by every project, so only [`Self::collect_assets`] brings an external one
    /// in, while [`Self::save_as`] carries across the ones this project already owns. This is the
    /// mechanism both of them use, so there is one account of what "the project owns it" means on
    /// disk.
    pub(super) fn collect_font(
        &mut self,
        id: SoundFontId,
        from: &Path,
    ) -> Result<(), SessionError> {
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;
        let name = copy_into(from, &folder.join(AUDIO_DIR))?;
        if let Some(font) = self.project.soundfonts.get_mut(&id) {
            font.path = AssetPath::inside(Path::new(AUDIO_DIR).join(name));
        }
        Ok(())
    }

    /// Records that an audio file turned out to be somewhere other than where it was stored.
    fn relocate_source(&mut self, id: SourceId, stored: &AssetPath, found: &Path) {
        let Some(reference) = self.moved_reference(stored, found) else {
            return;
        };
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.path = reference;
        }
        self.record_source_size(id, found);
        self.dirty = true;
    }

    /// Records that a SoundFont turned out to be somewhere other than where it was stored.
    fn relocate_font(&mut self, id: SoundFontId, stored: &AssetPath, found: &Path) {
        let Some(reference) = self.moved_reference(stored, found) else {
            return;
        };
        if let Some(font) = self.project.soundfonts.get_mut(&id) {
            font.path = reference;
            font.byte_size = byte_size(found);
        }
        self.dirty = true;
    }

    /// Writes down how large the file an audio source names is.
    ///
    /// The fingerprint `Session::reload_assets` confirms a candidate with, so it is refreshed
    /// everywhere the reference is rewritten — a font does the same thing inline in
    /// `Session::relocate_font`, and a source needs it in three places rather than one. A file
    /// that cannot be measured records 0, which means "no fingerprint" and leaves the name to be
    /// taken on trust: the same answer a document written before the field existed gives.
    pub(super) fn record_source_size(&mut self, id: SourceId, file: &Path) {
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.byte_size = byte_size(file);
        }
    }

    /// How the document should refer to a file now found at `found`, or `None` when that is
    /// already what it says and nothing needs writing back.
    fn moved_reference(&self, stored: &AssetPath, found: &Path) -> Option<AssetPath> {
        let reference = match self
            .project_folder()
            .and_then(|folder| found.strip_prefix(folder).ok())
        {
            Some(relative) => AssetPath::inside(relative),
            None => AssetPath::external(found),
        };
        (&reference != stored).then_some(reference)
    }

    /// Stores decoded audio, the peaks used to draw it, and the copy the graph will render.
    pub(super) fn install_source(&mut self, id: SourceId, buffer: Arc<AudioBuffer>) {
        let peaks = compute_peaks(self.gpu.as_deref(), &buffer, WAVEFORM_BUCKET);
        self.waveforms.insert(id, Arc::new(peaks));
        if let Some(at_rate) = source_at_rate(id, &buffer, self.render_bank_rate) {
            self.render_bank.insert(id, at_rate);
        }
        self.bank.insert(id, buffer);
    }

    /// Drops everything decoded: both audio banks, the waveform cache and the fonts.
    pub(super) fn clear_sources(&mut self) {
        self.bank = AudioSourceBank::new();
        self.render_bank = AudioSourceBank::new();
        self.waveforms.clear();
        self.fonts.clear();
    }
}

/// Where an asset's file actually is.
///
/// The stored reference when it is still true, and otherwise the first place a search turns up a
/// file of the right name — confirmed by `expected_size` where the document recorded one, so a
/// different file wearing the same name is not quietly adopted.
fn locate(
    stored: &AssetPath,
    folder: Option<&Path>,
    search: &[PathBuf],
    expected_size: u64,
) -> Option<PathBuf> {
    if let Some(direct) = stored.resolve(folder)
        && direct.is_file()
    {
        return Some(direct);
    }
    find_named(stored.file_name()?, search, expected_size)
}

/// Adds the directory holding `found` to the places later searches will look.
fn remember_directory(search: &mut Vec<PathBuf>, found: &Path) {
    let Some(directory) = found.parent() else {
        return;
    };
    if !search.iter().any(|known| known == directory) {
        search.push(directory.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{Scratch, session, write_tone};
    use auris_core::time::Ticks;

    #[test]
    fn an_audio_file_that_moved_next_to_the_project_is_found_again() {
        // A version 1 document names its audio absolutely. Copying the project folder to another
        // machine breaks that path, and the file sitting in `Audio/` is the obvious candidate.
        let scratch = Scratch::new("relocate");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();
        // Put the file where a collected project would have it, and break the stored path.
        std::fs::rename(
            scratch.join("kick.wav"),
            folder.join(AUDIO_DIR).join("kick.wav"),
        )
        .unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert!(missing.is_empty(), "the file is right there: {missing:?}");
        assert_eq!(
            reopened.project().audio_sources[&source].path,
            AssetPath::inside(Path::new(AUDIO_DIR).join("kick.wav")),
            "and finding it must be written down, so it is found once rather than every time"
        );
        assert!(
            reopened.is_dirty(),
            "the repair is an unsaved change like any other"
        );
    }

    #[test]
    fn a_sample_of_the_wrong_size_wearing_the_name_is_not_adopted() {
        // A plain `save` collects nothing, so the document points outside its own folder — and
        // that file has gone. Something else called `kick.wav` is sitting in `Audio/`. Playing
        // that instead of reporting the sample missing is a wrong answer nobody is told about,
        // and Collect Assets afterwards writes it into the document for good.
        let scratch = Scratch::new("decoy");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();

        std::fs::remove_file(scratch.join("kick.wav")).unwrap();
        write_tone(&folder.join(AUDIO_DIR).join("kick.wav"), 4_800);

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert_eq!(missing.len(), 1, "a different file is not the file");
        assert!(
            !reopened.project().audio_sources[&source].path.is_inside(),
            "and the reference must not be rewritten to point at the impostor"
        );
    }

    #[test]
    fn a_sample_of_the_right_size_wearing_the_name_is_still_found() {
        // The other half of the same rule: the size confirms a candidate, it must not veto one.
        // The copy in `Audio/` is a different file on disk holding the same bytes, which is what
        // a project someone copied folder-first looks like.
        let scratch = Scratch::new("twin");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();

        std::fs::copy(
            scratch.join("kick.wav"),
            folder.join(AUDIO_DIR).join("kick.wav"),
        )
        .unwrap();
        std::fs::remove_file(scratch.join("kick.wav")).unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert!(missing.is_empty(), "the file is right there: {missing:?}");
        assert_eq!(
            reopened.project().audio_sources[&source].path,
            AssetPath::inside(Path::new(AUDIO_DIR).join("kick.wav")),
            "and finding it is written down, so it is found once rather than every time"
        );
    }

    #[test]
    fn a_file_that_is_really_gone_is_reported_rather_than_guessed_at() {
        let scratch = Scratch::new("gone");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(&folder).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        session.save(&folder.join("MySong.auris")).unwrap();
        std::fs::remove_file(scratch.join("kick.wav")).unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert_eq!(missing.len(), 1, "the project opens, and says what is gone");
        assert_eq!(reopened.project().tracks.len(), 1);
    }
}
