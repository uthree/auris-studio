//! Persistent file identities for the application's shared SoundFont library.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SessionError;
use crate::settings::{read_config_text, write_config_bytes};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LibraryFont {
    pub path: PathBuf,
    pub name: String,
    pub byte_size: u64,
}

pub(crate) fn read(path: &Path) -> Result<Vec<LibraryFont>, SessionError> {
    let text = match read_config_text(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(failed(path, error)),
    };
    let fonts: Vec<LibraryFont> =
        serde_json::from_str(&text).map_err(|error| failed(path, error))?;
    if fonts.iter().any(|font| !font.path.is_absolute()) {
        return Err(failed(path, "SoundFont paths must be absolute"));
    }
    Ok(fonts)
}

pub(crate) fn register(
    library: &Path,
    path: &Path,
    name: String,
) -> Result<LibraryFont, SessionError> {
    let path = std::fs::canonicalize(path).map_err(|error| failed(path, error))?;
    let byte_size = std::fs::metadata(&path)
        .map_err(|error| failed(&path, error))?
        .len();
    let entry = LibraryFont {
        path,
        name,
        byte_size,
    };
    if let Some(parent) = library.parent() {
        std::fs::create_dir_all(parent).map_err(|error| failed(library, error))?;
    }
    // Reload under a cross-process lock so two windows cannot overwrite each other's imports.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(library.with_extension("lock"))
        .map_err(|error| failed(library, error))?;
    lock.lock().map_err(|error| failed(library, error))?;
    let mut fonts = read(library)?;
    if fonts.iter().any(|font| font == &entry) {
        return Ok(entry);
    }
    fonts.retain(|font| {
        font.path != entry.path
            && !(font.path.file_name() == entry.path.file_name()
                && font.byte_size == entry.byte_size
                && !font.path.is_file())
    });
    fonts.push(entry.clone());
    let bytes = serde_json::to_vec_pretty(&fonts).map_err(|error| failed(library, error))?;
    write_config_bytes(library, &bytes).map_err(|error| failed(library, error))?;
    Ok(entry)
}

/// Takes every entry naming `path` out of the catalogue, returning whether there was one.
///
/// Only the catalogue changes. The file is somebody's own and was never copied in, so it stays
/// where it lies, and a project whose tracks still play it goes on finding it by its own path.
pub(crate) fn unregister(library: &Path, path: &Path) -> Result<bool, SessionError> {
    let canonical = std::fs::canonicalize(path).ok();
    let names = |font: &LibraryFont| {
        font.path == path
            || canonical
                .as_deref()
                .is_some_and(|canonical| font.path == canonical)
    };
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(library.with_extension("lock"))
        .map_err(|error| failed(library, error))?;
    lock.lock().map_err(|error| failed(library, error))?;
    let mut fonts = read(library)?;
    let before = fonts.len();
    fonts.retain(|font| !names(font));
    if fonts.len() == before {
        return Ok(false);
    }
    let bytes = serde_json::to_vec_pretty(&fonts).map_err(|error| failed(library, error))?;
    write_config_bytes(library, &bytes).map_err(|error| failed(library, error))?;
    Ok(true)
}

fn failed(path: &Path, error: impl std::fmt::Display) -> SessionError {
    SessionError::SoundFontLibrary(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_imports_merge_instead_of_overwriting_the_catalog() {
        let scratch = tempfile::tempdir().unwrap();
        let library = scratch.path().join("soundfonts.json");
        let first = scratch.path().join("first.sf2");
        let second = scratch.path().join("second.sf2");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for path in [&first, &second] {
                let library = &library;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    register(library, path, "Font".into()).unwrap();
                });
            }
        });
        let fonts = read(&library).unwrap();
        assert_eq!(fonts.len(), 2);
        assert!(
            fonts
                .iter()
                .any(|font| font.path == std::fs::canonicalize(&first).unwrap())
        );
        assert!(
            fonts
                .iter()
                .any(|font| font.path == std::fs::canonicalize(&second).unwrap())
        );
    }

    #[test]
    fn unregistering_takes_out_one_entry_and_leaves_the_file() {
        let scratch = tempfile::tempdir().unwrap();
        let library = scratch.path().join("soundfonts.json");
        let first = scratch.path().join("first.sf2");
        let second = scratch.path().join("second.sf2");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        register(&library, &first, "First".into()).unwrap();
        register(&library, &second, "Second".into()).unwrap();

        assert!(unregister(&library, &first).unwrap());
        let fonts = read(&library).unwrap();
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].name, "Second");
        assert!(first.is_file(), "the file itself is never deleted");
        // A second attempt finds nothing to take out and writes nothing.
        assert!(!unregister(&library, &first).unwrap());
    }
}
