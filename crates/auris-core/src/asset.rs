//! How a document refers to a file it does not contain.
//!
//! Audio and SoundFonts are far too large to live in the document, so a project names them by
//! path. A bare absolute path is the weakest reference there is: it breaks when the folder moves,
//! when the project is copied to another machine, when a drive is mounted under a different
//! letter. [`AssetPath`] fixes the common half of that by distinguishing the two cases that want
//! opposite treatment.
//!
//! * [`AssetPath::Inside`] — the file lives in the project folder and is stored *relative* to it,
//!   so the whole folder can be moved, renamed, copied or zipped and still open.
//! * [`AssetPath::External`] — the file lives somewhere else and is stored absolutely, because
//!   nothing else would find it.
//!
//! Which one an asset gets is a policy decision, not a property of the file: imported audio
//! belongs to one song and is copied in, while a SoundFont is a shared library that would cost
//! hundreds of megabytes per project to duplicate. A moved external file is found again by
//! searching for it, which is why the document stores enough to recognise one.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Separator used inside a stored relative path, on every platform.
const PORTABLE_SEPARATOR: char = '/';

/// Where an asset's file is, as the document records it.
///
/// Comparison is on the stored form, so two references are equal when they say the same thing —
/// an `Inside` and an `External` reference that happen to resolve to the same file are not equal,
/// and should not be, since only one of them survives the folder being moved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "AssetPathRepr", into = "AssetPathRepr")]
pub enum AssetPath {
    /// Inside the project folder, at this relative path.
    Inside(PathBuf),
    /// Elsewhere on this machine, at this absolute path.
    External(PathBuf),
}

impl AssetPath {
    /// A reference to a file outside the project folder.
    pub fn external(path: impl Into<PathBuf>) -> Self {
        AssetPath::External(path.into())
    }

    /// A reference to a file in the project folder, relative to it.
    ///
    /// The path is stored with `/` separators whatever the platform handed in, and with anything
    /// that is not a plain directory or file name dropped. A project folder is meant to be copied
    /// between machines, and `Audio\kick.wav` is a *file called that* on a Unix system rather than
    /// a file in a directory — a project saved on Windows would arrive on a Mac with silent
    /// tracks. Dropping `..` along the way keeps "inside" true, which is the other half of the
    /// promise this variant makes.
    pub fn inside(relative: impl AsRef<Path>) -> Self {
        let portable: Vec<String> = relative
            .as_ref()
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        AssetPath::Inside(PathBuf::from(
            portable.join(&PORTABLE_SEPARATOR.to_string()),
        ))
    }

    /// Whether the file travels with the project folder.
    pub fn is_inside(&self) -> bool {
        matches!(self, AssetPath::Inside(_))
    }

    /// The path exactly as the document stores it: relative for `Inside`, absolute for `External`.
    ///
    /// For opening the file, use [`Self::resolve`]; this is for showing the reference and for
    /// reading its file name.
    pub fn as_stored(&self) -> &Path {
        match self {
            AssetPath::Inside(relative) => relative,
            AssetPath::External(absolute) => absolute,
        }
    }

    /// The file name, which is what identifies an asset once its path has stopped being true.
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.as_stored().file_name()
    }

    /// A path that can actually be opened, given the folder the document lives in.
    ///
    /// `None` only for an `Inside` reference with no folder to resolve against, which is a
    /// project that was never saved — and one of those cannot have collected anything, so the
    /// case exists to be handled rather than to happen.
    ///
    /// The relative path is rebuilt component by component rather than joined. `Path::join` with
    /// an absolute argument throws the folder away, and `Deserialize` does not go through
    /// [`Self::inside`] — so a hand-edited or hostile document could otherwise point an "inside"
    /// reference anywhere on the machine.
    pub fn resolve(&self, project_folder: Option<&Path>) -> Option<PathBuf> {
        match self {
            AssetPath::Inside(relative) => project_folder.map(|folder| {
                let mut resolved = folder.to_path_buf();
                for part in relative
                    .to_string_lossy()
                    .split([PORTABLE_SEPARATOR, '\\'])
                    .filter(|part| !part.is_empty() && *part != "." && *part != "..")
                {
                    resolved.push(part);
                }
                resolved
            }),
            AssetPath::External(absolute) => Some(absolute.clone()),
        }
    }
}

impl std::fmt::Display for AssetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_stored().display())
    }
}

/// On-disk shape of an [`AssetPath`].
///
/// Format version 1 wrote a bare path string, which meant "somewhere on this machine" — exactly
/// what [`AssetPath::External`] means now, so an old document needs no migration beyond being
/// read as one.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum AssetPathRepr {
    Located(Located),
    Legacy(PathBuf),
}

/// The tagged form written from format version 2 onwards.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Located {
    Inside(PathBuf),
    External(PathBuf),
}

impl From<AssetPathRepr> for AssetPath {
    fn from(repr: AssetPathRepr) -> Self {
        match repr {
            AssetPathRepr::Located(Located::Inside(relative)) => AssetPath::Inside(relative),
            AssetPathRepr::Located(Located::External(absolute)) => AssetPath::External(absolute),
            AssetPathRepr::Legacy(path) => AssetPath::External(path),
        }
    }
}

impl From<AssetPath> for AssetPathRepr {
    fn from(path: AssetPath) -> Self {
        AssetPathRepr::Located(match path {
            AssetPath::Inside(relative) => Located::Inside(relative),
            AssetPath::External(absolute) => Located::External(absolute),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inside_reference_follows_the_folder() {
        let asset = AssetPath::inside("Audio/kick.wav");
        assert_eq!(
            asset.resolve(Some(Path::new("/songs/first"))),
            Some(PathBuf::from("/songs/first/Audio/kick.wav"))
        );
        assert_eq!(
            asset.resolve(Some(Path::new("/backup/second"))),
            Some(PathBuf::from("/backup/second/Audio/kick.wav")),
            "the same document in a different folder must find its own copy"
        );
    }

    #[test]
    fn an_external_reference_ignores_the_folder() {
        let asset = AssetPath::external("/libraries/GM.sf2");
        assert_eq!(
            asset.resolve(Some(Path::new("/songs/first"))),
            Some(PathBuf::from("/libraries/GM.sf2"))
        );
        assert_eq!(
            asset.resolve(None),
            Some(PathBuf::from("/libraries/GM.sf2"))
        );
    }

    #[test]
    fn an_unsaved_project_cannot_resolve_an_inside_reference() {
        assert_eq!(AssetPath::inside("Audio/kick.wav").resolve(None), None);
    }

    #[test]
    fn both_forms_round_trip_through_json() {
        for asset in [
            AssetPath::inside("Audio/kick.wav"),
            AssetPath::external("/libraries/GM.sf2"),
        ] {
            let json = serde_json::to_string(&asset).unwrap();
            assert_eq!(serde_json::from_str::<AssetPath>(&json).unwrap(), asset);
        }
    }

    #[test]
    fn the_written_form_says_which_kind_it_is() {
        // The document is meant to be readable, and "inside" is the word that explains why the
        // path next to it has no drive letter.
        assert_eq!(
            serde_json::to_string(&AssetPath::inside("Audio/kick.wav")).unwrap(),
            r#"{"inside":"Audio/kick.wav"}"#
        );
        assert_eq!(
            serde_json::to_string(&AssetPath::external("/libraries/GM.sf2")).unwrap(),
            r#"{"external":"/libraries/GM.sf2"}"#
        );
    }

    #[test]
    fn a_version_one_path_reads_as_external() {
        let asset: AssetPath = serde_json::from_str(r#""/music/loops/kick.wav""#).unwrap();
        assert_eq!(asset, AssetPath::external("/music/loops/kick.wav"));
    }

    #[test]
    fn a_relative_path_is_stored_the_same_way_on_every_platform() {
        // Built from native components on Windows, this would store `Audio\kick.wav` — which on a
        // Mac is one file with a backslash in its name, and the track comes up silent.
        let native = Path::new("Audio").join("kick.wav");
        assert_eq!(
            serde_json::to_string(&AssetPath::inside(native)).unwrap(),
            r#"{"inside":"Audio/kick.wav"}"#
        );
    }

    #[test]
    fn an_inside_reference_cannot_point_outside() {
        // `Path::join` with an absolute path discards the base, and nothing stops a hand-edited
        // document from containing one.
        let folder = Path::new("/songs/first");
        for escape in ["/etc/passwd", "../../etc/passwd", "..\\..\\secrets"] {
            let stored: AssetPath =
                serde_json::from_value(serde_json::json!({ "inside": escape })).unwrap();
            let resolved = stored.resolve(Some(folder)).unwrap();
            assert!(
                resolved.starts_with(folder),
                "`{escape}` resolved to {}",
                resolved.display()
            );
        }
    }

    #[test]
    fn a_windows_separator_in_a_stored_path_still_resolves() {
        // Nothing this build writes looks like this, but a document from elsewhere might, and
        // finding the file beats being right about the format.
        let stored: AssetPath = serde_json::from_str(r#"{"inside":"Audio\\kick.wav"}"#).unwrap();
        assert_eq!(
            stored.resolve(Some(Path::new("/songs/first"))),
            Some(Path::new("/songs/first").join("Audio").join("kick.wav"))
        );
    }

    #[test]
    fn the_file_name_survives_the_path_going_stale() {
        // Searching for a moved file starts here, so it has to work for both forms.
        assert_eq!(
            AssetPath::inside("Audio/kick.wav").file_name().unwrap(),
            "kick.wav"
        );
        assert_eq!(
            AssetPath::external("/libraries/GM.sf2")
                .file_name()
                .unwrap(),
            "GM.sf2"
        );
    }
}
