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

use std::fs;
use std::io::Cursor;
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
///
/// The whole file is read first and its chunk tree walked before the parser is allowed to
/// believe it — see [`check_chunks`] for what the parser would otherwise do with a size field
/// that lies.
pub fn load_soundfont(path: &Path) -> Result<Arc<SoundFont>> {
    let bytes = fs::read(path)
        .map_err(|error| IoError::Decode(format!("could not open {}: {error}", path.display())))?;
    let refused = |reason: String| {
        IoError::Decode(format!(
            "{} is not a SoundFont this build can read: {reason}",
            path.display()
        ))
    };
    check_chunks(&bytes).map_err(&refused)?;
    let font =
        SoundFont::new(&mut Cursor::new(bytes)).map_err(|error| refused(error.to_string()))?;
    Ok(Arc::new(font))
}

/// Walks the chunk tree the way the parser will, checking every size field against the bytes
/// that are really in the file.
///
/// `rustysynth` believes sizes. A chunk claiming more bytes than the file holds is *allocated*
/// before the read that would notice — a size near `u32::MAX` is a four-gigabyte allocation
/// that aborts the process instead of returning an error. And the `smpl` chunk is read by
/// viewing a buffer of 16-bit samples as bytes: an odd size makes that view one byte longer
/// than the allocation, which is undefined behaviour, not a parse error. Both stop here.
///
/// A file that is not RIFF at all is let through: the parser refuses those on its own, safely
/// and naming what it expected. What must not reach it is a *plausible* file whose sizes lie.
fn check_chunks(bytes: &[u8]) -> std::result::Result<(), String> {
    if bytes.len() < 12 || !bytes.starts_with(b"RIFF") {
        return Ok(());
    }
    // Past `RIFF`, the size field the parser never reads, and `sfbk`. The parser reads exactly
    // three lists — INFO, sdta, pdta — and never looks at anything after them, so neither does
    // this walk.
    let mut at = 12;
    for _ in 0..3 {
        at = check_chunk(bytes, at, true)?;
    }
    Ok(())
}

/// Checks the chunk at `at`, descending into a `LIST`, and returns where the parser's cursor
/// lands afterwards.
///
/// Running out of file is `Ok` — the parser turns plain truncation into an error safely. The
/// walk mirrors the parser's stream reads exactly, pad bytes included (there are none):
/// stricter, and it would refuse fonts the parser plays; looser, and a lying size gets through.
fn check_chunk(bytes: &[u8], at: usize, descend: bool) -> std::result::Result<usize, String> {
    let Some(header) = bytes.get(at..at + 8) else {
        return Ok(bytes.len());
    };
    let id: [u8; 4] = header[..4].try_into().expect("sliced four bytes");
    let size = u32::from_le_bytes(header[4..8].try_into().expect("sliced four bytes")) as usize;
    let body = at + 8;
    let held = bytes.len() - body;
    if size > held {
        return Err(format!(
            "chunk {} says it is {size} bytes long, but only {held} bytes follow it",
            String::from_utf8_lossy(&id),
        ));
    }
    if id == *b"smpl" && !size.is_multiple_of(2) {
        return Err(format!("the sample data is an odd {size} bytes long"));
    }
    if descend && id == *b"LIST" && size >= 4 {
        // Children are read while the parser's byte count sits short of the list's claimed
        // size; the count starts after the four-byte list type, and a child that overruns the
        // claim leaves the cursor wherever it stopped rather than at the claimed end.
        let end = body + size;
        let mut child = body + 4;
        while child < end {
            child = check_chunk(bytes, child, false)?;
        }
        return Ok(child);
    }
    Ok(body + size)
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

/// How many sounds a font offers.
///
/// Separate from [`presets`] because a list that is only being *counted* — a library row saying
/// how much is inside a font it has not opened — should not build and sort several hundred
/// strings to arrive at a number.
pub fn preset_count(font: &SoundFont) -> usize {
    font.get_presets().len()
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
    use std::fs::File;
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

    // ---------------------------------------------------------------- the size walk

    /// A chunk with an honest size field.
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A LIST holding `children`, typed `kind`.
    fn list(kind: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let mut body = kind.to_vec();
        body.extend_from_slice(children);
        chunk(b"LIST", &body)
    }

    /// A RIFF file of `lists`, sized honestly.
    fn font_file(lists: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"sfbk".to_vec();
        for entry in lists {
            body.extend_from_slice(entry);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    /// The smallest structure shaped like a font: three lists, an even run of samples.
    fn skeleton() -> Vec<Vec<u8>> {
        vec![
            list(b"INFO", &chunk(b"ifil", &[2, 0, 1, 0])),
            list(b"sdta", &chunk(b"smpl", &[0; 8])),
            list(b"pdta", &chunk(b"phdr", &[0; 76])),
        ]
    }

    #[test]
    fn a_well_formed_skeleton_walks_clean() {
        assert_eq!(check_chunks(&font_file(&skeleton())), Ok(()));
    }

    #[test]
    fn an_odd_sample_chunk_is_refused_before_the_parser_believes_it() {
        // The parser reads `smpl` by viewing a buffer of 16-bit samples as bytes; an odd size
        // makes that view one byte longer than the allocation. That is undefined behaviour
        // inside a file-dialog callback, and it has to become this error instead.
        let file = font_file(&[
            list(b"INFO", &chunk(b"ifil", &[2, 0, 1, 0])),
            list(b"sdta", &chunk(b"smpl", &[1, 2, 3, 4, 5])),
        ]);
        let temp = TempFile::new("odd-samples.sf2");
        std::fs::write(temp.path(), file).expect("write");

        let error = load_soundfont(temp.path()).unwrap_err();
        assert!(error.to_string().contains("odd-samples.sf2"));
        assert!(
            error.to_string().contains("odd"),
            "the message should say what lied: {error}"
        );
    }

    #[test]
    fn a_chunk_claiming_more_than_the_file_holds_is_refused() {
        // The claimed size is allocated before the read that would notice it lies, so a size
        // near u32::MAX is a four-gigabyte allocation — which aborts, rather than erroring.
        let mut sample = chunk(b"smpl", &[0; 4]);
        sample[4..8].copy_from_slice(&4_000_000_000_u32.to_le_bytes());
        let mut children = chunk(b"ifil", &[2, 0, 1, 0]);
        children.extend(sample);
        let file = font_file(&[list(b"INFO", &children)]);
        let temp = TempFile::new("greedy.sf2");
        std::fs::write(temp.path(), file).expect("write");

        let error = load_soundfont(temp.path()).unwrap_err();
        assert!(error.to_string().contains("greedy.sf2"));
        assert!(
            error.to_string().contains("4000000000"),
            "the message should say what was claimed: {error}"
        );
    }

    #[test]
    fn what_is_not_riff_at_all_is_left_for_the_parser_to_refuse() {
        // The parser's own refusal names what it expected, which is the better message; the
        // walk only has to stop the files the parser would *believe*.
        assert_eq!(check_chunks(b"this is not a RIFF container"), Ok(()));
        assert_eq!(check_chunks(b""), Ok(()));
    }

    #[test]
    fn what_the_parser_never_reads_is_not_judged() {
        // The parser ignores the RIFF chunk's own size field and stops after the third list,
        // so a wrong size there and rubbish after the lists must not fail a font it plays.
        let mut file = font_file(&skeleton());
        file[4..8].copy_from_slice(&7_u32.to_le_bytes());
        file.extend_from_slice(b"trailing rubbish no parser will see");
        assert_eq!(check_chunks(&file), Ok(()));
    }

    #[test]
    fn a_truncated_chunk_header_is_left_for_the_parser_too() {
        // Plain truncation becomes the parser's own read error, safely; the walk stops early
        // rather than inventing a second opinion. Cut inside the third list's header, so no
        // surviving size field is made a liar by the cut.
        let pdta = list(b"pdta", &chunk(b"phdr", &[0; 76]));
        let mut file = font_file(&skeleton());
        file.truncate(file.len() - pdta.len() + 5);
        assert_eq!(check_chunks(&file), Ok(()));
    }
}
