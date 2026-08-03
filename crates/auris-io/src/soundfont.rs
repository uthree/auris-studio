//! Reading SoundFont files.
//!
//! An SF2 is a RIFF container holding sample data, the regions that map keys and velocities onto
//! it, and the envelopes and filters each region is played through. Parsing one and *playing* one
//! are a long way apart, so this crate does neither itself: `rustysynth` does both, and what is
//! here is the boundary — opening the file, turning its failures into this crate's error type,
//! and describing what is inside it in terms that do not mention the library.
//!
//! The file is not held open. A font is read once into memory and shared by `Arc` from there,
//! which is the same bargain [`crate::import`] makes with audio files: the document keeps a path,
//! the samples live beside it at runtime, and a project stays small enough to read.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rustysynth::SoundFont;

use crate::error::{IoError, Result};

/// File extensions the SoundFont importer accepts, for a file-picker filter.
pub fn soundfont_extensions() -> &'static [&'static str] {
    &["sf2"]
}

/// One playable sound in a font.
///
/// Named by bank and patch rather than by position, because that pair is what a project stores
/// and what selects the sound again after the font is reloaded — a position would move the moment
/// anyone edited the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundFontPreset {
    /// MIDI bank, 0 for the standard set and 128 for percussion.
    pub bank: i32,
    /// MIDI program number within the bank.
    pub patch: i32,
    /// What the font calls it.
    pub name: String,
}

/// Reads a SoundFont into memory.
///
/// Shared by `Arc` because one font backs every track that plays it: a 200 MB orchestral set
/// loaded once and referenced eight times is the difference between a project that opens and one
/// that does not.
pub fn load_soundfont(path: &Path) -> Result<Arc<SoundFont>> {
    let file = File::open(path)
        .map_err(|error| IoError::Decode(format!("could not open {}: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let font = SoundFont::new(&mut reader).map_err(|error| {
        IoError::Decode(format!(
            "{} is not a SoundFont this build can read: {error}",
            path.display()
        ))
    })?;
    Ok(Arc::new(font))
}

/// Every preset a font offers, in bank and patch order.
///
/// Sorted rather than left in file order: a font is free to store its presets however it likes,
/// and a list that changed order between two fonts would be a list nobody could learn.
pub fn presets(font: &SoundFont) -> Vec<SoundFontPreset> {
    let mut presets: Vec<SoundFontPreset> = font
        .get_presets()
        .iter()
        .map(|preset| SoundFontPreset {
            bank: preset.get_bank_number(),
            patch: preset.get_patch_number(),
            name: preset.get_name().trim().to_string(),
        })
        .collect();
    presets.sort_by_key(|preset| (preset.bank, preset.patch));
    presets
}

/// What a font calls itself, or the file's own stem when it says nothing useful.
///
/// Fonts in the wild routinely carry an empty name or a leftover like `Untitled`, and a library
/// listing several of those is a library with nothing to choose between.
pub fn font_name(font: &SoundFont, path: &Path) -> String {
    let stated = font.get_info().get_bank_name().trim();
    if !stated.is_empty() && !stated.eq_ignore_ascii_case("untitled") {
        return stated.to_string();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "SoundFont".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;
    use std::io::Write;

    #[test]
    fn a_path_that_is_not_there_is_an_error_rather_than_a_panic() {
        let missing = Path::new("no-such-file-anywhere.sf2");
        let error = load_soundfont(missing).unwrap_err();
        assert!(
            error.to_string().contains("no-such-file-anywhere.sf2"),
            "the message should name the file: {error}"
        );
    }

    #[test]
    fn a_file_that_is_not_a_soundfont_is_refused_by_name() {
        // Whatever the parser makes of the bytes, the caller has to get an error it can show —
        // a panic here would be inside a file dialog's callback.
        let temp = TempFile::new("not-a-font.sf2");
        let mut file = File::create(temp.path()).expect("temp file");
        file.write_all(b"this is not a RIFF container at all")
            .expect("write");
        drop(file);

        let error = load_soundfont(temp.path()).unwrap_err();
        assert!(error.to_string().contains("not-a-font.sf2"));
    }

    #[test]
    fn an_empty_file_is_refused_too() {
        let temp = TempFile::new("empty.sf2");
        File::create(temp.path()).expect("temp file");
        assert!(load_soundfont(temp.path()).is_err());
    }

    #[test]
    fn the_extension_filter_names_what_the_importer_reads() {
        assert_eq!(soundfont_extensions(), &["sf2"]);
    }
}
