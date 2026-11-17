//! Taking a SoundFont back off the shelf.
//!
//! A font is imported into the application's library, not into one song, so removing it is a
//! library operation too: it stops being offered to every project, and — like importing it — it
//! is not an edit of the open document and leaves no undo step. Two things are deliberately left
//! alone. The file is the person's own and was referred to where it lies, so it is never deleted;
//! and a track that plays one of its sounds keeps it, because silencing a part of somebody's song
//! is not what taking a font off a shelf means. A document therefore holds a font for one of three
//! reasons only: the library has it, a track plays it, or it was collected into the project folder.

use std::path::{Path, PathBuf};

use auris_core::{Project, SoundFontId, TrackId};
use auris_sampler::stored_preset;

use super::Session;
use crate::error::SessionError;

impl Session {
    /// `true` when [`Self::remove_soundfont`] can take this font out of the library.
    ///
    /// `false` for a font the project does not name, and for one that came with this
    /// installation: that is what the installation *has*, and the next launch would only put it
    /// back.
    pub fn soundfont_is_removable(&self, id: SoundFontId) -> bool {
        self.project
            .soundfonts
            .get(&id)
            .is_some_and(|font| !is_shipped(font.path.file_name()))
    }

    /// The tracks of the open project whose stored sound names this font, in track order.
    ///
    /// What a confirmation should name before [`Self::remove_soundfont`]: these tracks keep the
    /// font in this project after it has left the library.
    pub fn soundfont_users(&self, id: SoundFontId) -> Vec<TrackId> {
        users(&self.project, id)
    }

    /// Removes a SoundFont from the application library and from every document that does not
    /// play it.
    ///
    /// Returns how many tracks of the open project still play it. Those keep it in this project,
    /// so it stays listed here; every other project stops being offered it, and one opened later
    /// drops it unless its own tracks play it. The file is never deleted.
    ///
    /// Not an edit, for the reason importing is not one: the library is the installation's, not
    /// the document's. The dirty flag is left as it was and no undo step is recorded; an undo
    /// snapshot whose tracks play the font keeps it, so Undo still sounds.
    pub fn remove_soundfont(&mut self, id: SoundFontId) -> Result<usize, SessionError> {
        let reference = self
            .project
            .soundfonts
            .get(&id)
            .ok_or(SessionError::UnknownSoundFont(id.0))?;
        if is_shipped(reference.path.file_name()) {
            return Err(SessionError::ShippedSoundFont(id.0));
        }
        let path = reference.path.resolve(self.project_folder());
        // The catalogue first: a failed write must not leave a font that looks removed here and
        // comes back at the next launch.
        if let (Some(library), Some(path)) = (&self.soundfont_library, &path) {
            crate::library::soundfonts::unregister(library, path)?;
        }
        let kept = users(&self.project, id).len();
        match path {
            Some(path) => self.withdraw_font(&path),
            None => {
                // Nothing on disk to compare with: only this reference can be meant.
                if kept == 0 {
                    self.project.soundfonts.remove(&id);
                    self.fonts.remove(id);
                }
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(kept)
    }

    /// Drops references no track plays from a document that has a library to answer for them.
    ///
    /// The other half of [`Self::remove_soundfont`]: every saved project names each font that
    /// was in the library when it was saved, so without this a removed font would come back the
    /// moment an older song was opened. Shipped fonts and fonts collected into the project folder
    /// are never touched.
    pub(super) fn drop_unlisted_fonts(&mut self, listed: &[PathBuf]) {
        let folder = self.project_folder().map(Path::to_path_buf);
        let unlisted: Vec<PathBuf> = self
            .project
            .soundfonts
            .values()
            .filter(|font| !font.path.is_inside() && !is_shipped(font.path.file_name()))
            .filter_map(|font| font.path.resolve(folder.as_deref()))
            .filter(|path| {
                !listed
                    .iter()
                    .any(|entry| super::files::same_font_file(entry, path))
            })
            .collect();
        for path in unlisted {
            self.withdraw_font(&path);
        }
    }

    /// Removes the references to `path` that no track plays, from the document and from every
    /// undo branch, judged branch by branch.
    fn withdraw_font(&mut self, path: &Path) {
        let folder = self.project_folder().map(Path::to_path_buf);
        let names = |project: &Project| -> Vec<SoundFontId> {
            project
                .soundfonts
                .values()
                .filter(|font| {
                    font.path
                        .resolve(folder.as_deref())
                        .is_some_and(|file| super::files::same_font_file(&file, path))
                })
                .map(|font| font.id)
                .collect()
        };
        let withdraw = |project: &mut Project| {
            for id in names(project) {
                if users(project, id).is_empty() {
                    project.soundfonts.remove(&id);
                }
            }
        };
        let before = names(&self.project);
        withdraw(&mut self.project);
        for id in before {
            if !self.project.soundfonts.contains_key(&id) {
                self.fonts.remove(id);
            }
        }
        self.history.for_each_project_mut(withdraw);
        withdraw(&mut self.saved_edit_project);
        if let Some(transaction) = &mut self.transaction {
            withdraw(&mut transaction.before);
        }

        // The samples stay cached only while some branch can still play them, and stop being
        // shared so that a new document does not inherit them.
        let mut named =
            !names(&self.project).is_empty() || !names(&self.saved_edit_project).is_empty();
        self.history.for_each_project_mut(|project| {
            named |= !names(project).is_empty();
        });
        if let Some(transaction) = &mut self.transaction {
            named |= !names(&transaction.before).is_empty();
        }
        let cached: Vec<PathBuf> = self
            .font_cache
            .keys()
            .filter(|cached| super::files::same_font_file(cached, path))
            .cloned()
            .collect();
        for cached in cached {
            if named {
                if let Some(font) = self.font_cache.get_mut(&cached) {
                    font.shared = false;
                }
            } else {
                self.font_cache.remove(&cached);
            }
        }
    }
}

/// The tracks whose stored sound names `font`, whatever instrument they currently have.
///
/// Wider than [`Session::track_preset`] on purpose: a track switched to another instrument keeps
/// its preset in its state, and switching back would find the font gone.
fn users(project: &Project, font: SoundFontId) -> Vec<TrackId> {
    project
        .tracks
        .iter()
        .filter(|track| {
            track.kind.as_instrument().is_some_and(|instrument| {
                stored_preset(&instrument.instrument_state)
                    .is_some_and(|preset| preset.font == font)
            })
        })
        .map(|track| track.id)
        .collect()
}

/// Whether a font file is one this installation ships, wherever it was installed.
fn is_shipped(file: Option<&std::ffi::OsStr>) -> bool {
    file.is_some_and(|file| {
        crate::library::SHIPPED
            .iter()
            .any(|font| file == std::ffi::OsStr::new(font.file))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use auris_core::PresetRef;

    use super::super::fixtures::Scratch;
    use super::super::{Session, SessionOptions};

    fn with_library(path: &Path) -> Session {
        Session::new(SessionOptions::headless().with_soundfont_library(path)).unwrap()
    }

    #[test]
    fn an_unused_font_leaves_the_library_and_the_document_but_not_the_disk() {
        let scratch = Scratch::new("remove-font-unused");
        let library = scratch.join("soundfonts.json");
        let path = scratch.soundfont("Piano.sf2");
        let mut session = with_library(&library);
        session.forget_history();
        let font = session.import_soundfont(&path).unwrap();
        assert!(session.soundfont_is_removable(font));

        assert_eq!(session.remove_soundfont(font).unwrap(), 0);
        assert_eq!(session.soundfonts().count(), 0);
        assert!(!session.soundfont_is_loaded(font));
        assert!(
            crate::library::soundfonts::read(&library)
                .unwrap()
                .is_empty()
        );
        assert!(path.is_file());
        assert!(!session.is_dirty());
        assert!(!session.can_undo());

        // Gone from the next document and the next launch as well.
        session.new_project();
        assert_eq!(session.soundfonts().count(), 0);
        drop(session);
        assert_eq!(with_library(&library).soundfonts().count(), 0);
    }

    #[test]
    fn a_font_a_track_plays_stays_in_that_project_only() {
        let scratch = Scratch::new("remove-font-used");
        let library = scratch.join("soundfonts.json");
        let path = scratch.soundfont("Piano.sf2");
        let mut session = with_library(&library);
        let font = session.import_soundfont(&path).unwrap();
        let track = session.add_default_instrument_track("Piano").unwrap();
        let preset = PresetRef {
            font,
            bank: 0,
            patch: 0,
        };
        session.set_track_preset(track, preset).unwrap();
        assert_eq!(session.soundfont_users(font), vec![track]);

        assert_eq!(session.remove_soundfont(font).unwrap(), 1);
        assert!(session.soundfont_is_loaded(font));
        assert_eq!(session.track_preset(track), Some(preset));
        assert!(
            crate::library::soundfonts::read(&library)
                .unwrap()
                .is_empty()
        );

        session.new_project();
        assert_eq!(session.soundfonts().count(), 0);
    }

    #[test]
    fn an_older_song_does_not_bring_a_removed_font_back() {
        let scratch = Scratch::new("remove-font-reopen");
        let library = scratch.join("soundfonts.json");
        let piano = scratch.soundfont("Piano.sf2");
        let organ = scratch.soundfont("Organ.sf2");
        let mut session = with_library(&library);
        let unused = session.import_soundfont(&piano).unwrap();
        let used = session.import_soundfont(&organ).unwrap();
        let track = session.add_default_instrument_track("Organ").unwrap();
        session
            .set_track_preset(
                track,
                PresetRef {
                    font: used,
                    bank: 0,
                    patch: 0,
                },
            )
            .unwrap();
        session.save_as(&scratch.join("Song.auris")).unwrap();
        let saved = session.path().unwrap().to_path_buf();
        session.remove_soundfont(unused).unwrap();
        session.remove_soundfont(used).unwrap();
        drop(session);

        let mut reopened = with_library(&library);
        reopened.open(&saved).unwrap();
        let names: Vec<_> = reopened.soundfonts().map(|font| font.id).collect();
        assert_eq!(names, vec![used], "only the font a track plays is kept");
        assert!(reopened.soundfont_is_loaded(used));
        assert!(!reopened.is_dirty());
    }

    #[test]
    fn undo_into_a_branch_that_played_the_font_still_sounds() {
        let scratch = Scratch::new("remove-font-undo");
        let mut session = with_library(&scratch.join("soundfonts.json"));
        let font = session
            .import_soundfont(&scratch.soundfont("Piano.sf2"))
            .unwrap();
        let track = session.add_default_instrument_track("Piano").unwrap();
        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 0,
                },
            )
            .unwrap();
        session.remove_track(track).unwrap();
        assert!(session.soundfont_users(font).is_empty());

        assert_eq!(session.remove_soundfont(font).unwrap(), 0);
        assert_eq!(session.soundfonts().count(), 0);
        session.undo().unwrap();
        assert!(session.project().track(track).is_some());
        assert!(session.soundfont_is_loaded(font));
    }

    #[test]
    fn an_unknown_font_is_refused() {
        let scratch = Scratch::new("remove-font-unknown");
        let mut session = with_library(&scratch.join("soundfonts.json"));
        let missing = auris_core::SoundFontId(4242);
        assert!(!session.soundfont_is_removable(missing));
        assert!(matches!(
            session.remove_soundfont(missing),
            Err(crate::SessionError::UnknownSoundFont(4242))
        ));
    }
}
