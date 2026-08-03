//! Moving an asset into a project folder, and finding one that has moved out from under it.
//!
//! The document decides *whether* an asset should be collected — that is a policy about what a
//! song owns, and it lives in the session. This module is the mechanism underneath: copy a file
//! in without ever destroying one already there, and look for a file that is no longer where it
//! was last seen.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{IoError, Result};

/// Chunk size for comparing two files. Large enough that the read syscall is not the cost, small
/// enough that two of them are nothing next to the audio buffers this process already holds.
const COMPARE_CHUNK: usize = 64 * 1024;

/// How many names to try before giving up on placing a file.
///
/// A thousand distinct files called `kick.wav` in one project means something has gone wrong
/// upstream, and looping forever would hide it.
const MAX_ATTEMPTS: u32 = 1000;

/// Size of a file in bytes, or 0 when it cannot be read.
///
/// 0 for "unknown" rather than an error: this is only ever used to *recognise* a file later, and
/// a missing fingerprint should weaken that check, not fail the operation that recorded it.
pub fn byte_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Copies `file` into `directory`, returning the file name it ended up under.
///
/// The name is returned rather than the full path because that is what the document stores: a
/// reference relative to the project folder, so the folder can be moved afterwards.
///
/// A name already taken in `directory` is never overwritten. If the file there has the same
/// contents the copy is skipped and the existing name returned — re-collecting a project is then
/// free, and a file already inside the folder is recognised as itself. Otherwise the new file is
/// placed under `kick-2.wav`, `kick-3.wav` and so on, because two different drum hits that happen
/// to share a name are still two drum hits.
pub fn copy_into(file: &Path, directory: &Path) -> Result<OsString> {
    std::fs::create_dir_all(directory).map_err(|error| IoError::from_fs(directory, error))?;

    let name = file
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from("asset"));

    for attempt in 1..=MAX_ATTEMPTS {
        let candidate = numbered(&name, attempt);
        let target = directory.join(&candidate);
        if !target.exists() {
            std::fs::copy(file, &target).map_err(|error| IoError::from_fs(file, error))?;
            return Ok(candidate);
        }
        if same_contents(file, &target)? {
            return Ok(candidate);
        }
    }

    Err(IoError::Filesystem {
        path: directory.join(&name),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{MAX_ATTEMPTS} files here are already called this"),
        ),
    })
}

/// Looks for `file_name` directly inside each of `directories`, in order.
///
/// `expected_size` confirms a candidate is the file that was lost rather than a different one
/// wearing its name; 0 means the document recorded no size and any match has to be taken on
/// trust. The search is deliberately not recursive — walking a home directory to find a sample
/// would be slow, surprising, and would happily return something from a backup nobody meant to
/// open.
pub fn find_named(
    file_name: &OsStr,
    directories: &[PathBuf],
    expected_size: u64,
) -> Option<PathBuf> {
    directories
        .iter()
        .map(|directory| directory.join(file_name))
        .find(|candidate| match std::fs::metadata(candidate) {
            Ok(meta) => meta.is_file() && (expected_size == 0 || meta.len() == expected_size),
            Err(_) => false,
        })
}

/// `kick.wav` for attempt 1, then `kick-2.wav`, `kick-3.wav`, …
fn numbered(name: &OsStr, attempt: u32) -> OsString {
    if attempt == 1 {
        return name.to_os_string();
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().unwrap_or(name);
    let mut numbered = stem.to_os_string();
    numbered.push(format!("-{attempt}"));
    if let Some(extension) = as_path.extension() {
        numbered.push(".");
        numbered.push(extension);
    }
    numbered
}

/// Whether two files hold the same bytes.
///
/// Size settles it in almost every case; when it does not, the contents are read. Two takes of
/// the same length recorded at the same settings have identical sizes, so stopping at the size
/// would sometimes quietly replace one with the other — and reading two files at save time is a
/// cheap price for never doing that.
fn same_contents(left: &Path, right: &Path) -> Result<bool> {
    let left_meta = std::fs::metadata(left).map_err(|error| IoError::from_fs(left, error))?;
    let right_meta = std::fs::metadata(right).map_err(|error| IoError::from_fs(right, error))?;
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }

    let mut left_file = std::io::BufReader::new(
        std::fs::File::open(left).map_err(|error| IoError::from_fs(left, error))?,
    );
    let mut right_file = std::io::BufReader::new(
        std::fs::File::open(right).map_err(|error| IoError::from_fs(right, error))?,
    );
    let mut left_chunk = vec![0u8; COMPARE_CHUNK];
    let mut right_chunk = vec![0u8; COMPARE_CHUNK];

    loop {
        let read =
            read_up_to(&mut left_file, &mut left_chunk).map_err(|e| IoError::from_fs(left, e))?;
        let matched = read_up_to(&mut right_file, &mut right_chunk)
            .map_err(|e| IoError::from_fs(right, e))?;
        if read != matched {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
        if left_chunk[..read] != right_chunk[..read] {
            return Ok(false);
        }
    }
}

/// Fills as much of `buffer` as the file has left, returning how many bytes that was.
///
/// `Read::read` may stop short of a full buffer for reasons that have nothing to do with the end
/// of the file, and a short read on one side would otherwise read as a difference.
fn read_up_to(source: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;

    /// A directory in the temp area that deletes itself, with helpers for putting files in it.
    struct TempDir {
        file: TempFile,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let file = TempFile::new(name);
            std::fs::create_dir_all(file.path()).unwrap();
            Self { file }
        }

        fn path(&self) -> &Path {
            self.file.path()
        }

        fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.path().join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.file.path());
        }
    }

    #[test]
    fn a_file_lands_under_its_own_name() {
        let source = TempDir::new("copy-source");
        let project = TempDir::new("copy-project");
        let file = source.write("kick.wav", b"one");

        let name = copy_into(&file, &project.path().join("Audio")).unwrap();
        assert_eq!(name, OsString::from("kick.wav"));
        assert_eq!(
            std::fs::read(project.path().join("Audio").join("kick.wav")).unwrap(),
            b"one"
        );
    }

    #[test]
    fn the_destination_directory_is_created() {
        let source = TempDir::new("mkdir-source");
        let project = TempDir::new("mkdir-project");
        let file = source.write("kick.wav", b"one");

        let audio = project.path().join("Audio");
        assert!(!audio.exists());
        copy_into(&file, &audio).unwrap();
        assert!(audio.is_dir());
    }

    #[test]
    fn copying_the_same_file_again_costs_nothing() {
        let source = TempDir::new("again-source");
        let project = TempDir::new("again-project");
        let file = source.write("kick.wav", b"one");

        assert_eq!(copy_into(&file, project.path()).unwrap(), "kick.wav");
        assert_eq!(
            copy_into(&file, project.path()).unwrap(),
            "kick.wav",
            "the file is already there and is the same file"
        );
        assert_eq!(std::fs::read_dir(project.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_already_inside_recognises_itself() {
        // What re-collecting an assembled project does: the source and the destination are one
        // file, and copying it over itself would be at best pointless.
        let project = TempDir::new("self-project");
        let file = project.write("kick.wav", b"one");

        assert_eq!(copy_into(&file, project.path()).unwrap(), "kick.wav");
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
    }

    #[test]
    fn a_different_file_of_the_same_name_does_not_replace_it() {
        let source = TempDir::new("clash-source");
        let project = TempDir::new("clash-project");
        project.write("kick.wav", b"first");
        let other = source.write("kick.wav", b"second");

        assert_eq!(copy_into(&other, project.path()).unwrap(), "kick-2.wav");
        assert_eq!(
            std::fs::read(project.path().join("kick.wav")).unwrap(),
            b"first",
            "the file that was there first must survive"
        );
        assert_eq!(
            std::fs::read(project.path().join("kick-2.wav")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn two_files_of_the_same_length_are_still_two_files() {
        // The case that rules out settling for a size comparison: two takes of the same length,
        // recorded at the same settings, are byte-for-byte different and must both survive.
        let source = TempDir::new("length-source");
        let project = TempDir::new("length-project");
        project.write("take.wav", b"aaaa");
        let other = source.write("take.wav", b"bbbb");

        assert_eq!(copy_into(&other, project.path()).unwrap(), "take-2.wav");
    }

    #[test]
    fn a_file_larger_than_one_chunk_compares_correctly() {
        let source = TempDir::new("big-source");
        let project = TempDir::new("big-project");
        let contents = vec![7u8; COMPARE_CHUNK * 2 + 13];

        let same = source.write("big.wav", &contents);
        project.write("big.wav", &contents);
        assert_eq!(
            copy_into(&same, project.path()).unwrap(),
            "big.wav",
            "identical past the first chunk is still identical"
        );

        // A difference in the very last byte has to be caught too, or the tail of a long file
        // could be silently replaced by a different take of the same length.
        let mut altered = contents.clone();
        *altered.last_mut().unwrap() = 8;
        let elsewhere = TempDir::new("big-other");
        let changed = elsewhere.write("big.wav", &altered);
        assert_eq!(copy_into(&changed, project.path()).unwrap(), "big-2.wav");
    }

    #[test]
    fn a_moved_file_is_found_by_name_and_size() {
        let library = TempDir::new("find-library");
        let elsewhere = TempDir::new("find-elsewhere");
        library.write("GM.sf2", b"the font");

        let found = find_named(
            OsStr::new("GM.sf2"),
            &[elsewhere.path().to_path_buf(), library.path().to_path_buf()],
            8,
        );
        assert_eq!(found, Some(library.path().join("GM.sf2")));
    }

    #[test]
    fn a_different_file_wearing_the_name_is_not_accepted() {
        let library = TempDir::new("wrong-library");
        library.write("GM.sf2", b"a different font entirely");

        assert_eq!(
            find_named(OsStr::new("GM.sf2"), &[library.path().to_path_buf()], 8),
            None
        );
    }

    #[test]
    fn an_unrecorded_size_takes_the_name_on_trust() {
        // Version 1 documents have no size to check against, and refusing to look for their
        // files at all would be worse than finding the wrong one.
        let library = TempDir::new("trust-library");
        library.write("GM.sf2", b"a font of some size");

        assert_eq!(
            find_named(OsStr::new("GM.sf2"), &[library.path().to_path_buf()], 0),
            Some(library.path().join("GM.sf2"))
        );
    }

    #[test]
    fn the_earlier_directory_wins() {
        let first = TempDir::new("order-first");
        let second = TempDir::new("order-second");
        first.write("GM.sf2", b"the font");
        second.write("GM.sf2", b"the font");

        assert_eq!(
            find_named(
                OsStr::new("GM.sf2"),
                &[first.path().to_path_buf(), second.path().to_path_buf()],
                8
            ),
            Some(first.path().join("GM.sf2"))
        );
    }

    #[test]
    fn nothing_is_found_when_nothing_is_there() {
        let empty = TempDir::new("empty-library");
        assert_eq!(
            find_named(OsStr::new("GM.sf2"), &[empty.path().to_path_buf()], 0),
            None
        );
        assert_eq!(find_named(OsStr::new("GM.sf2"), &[], 0), None);
    }

    #[test]
    fn a_directory_is_not_mistaken_for_the_file() {
        let library = TempDir::new("dir-library");
        std::fs::create_dir(library.path().join("GM.sf2")).unwrap();
        assert_eq!(
            find_named(OsStr::new("GM.sf2"), &[library.path().to_path_buf()], 0),
            None
        );
    }

    #[test]
    fn the_size_of_a_missing_file_is_zero() {
        assert_eq!(byte_size(Path::new("/no/such/file.sf2")), 0);
    }
}
