//! Saving and loading `.auris` project documents.
//!
//! Projects are stored as pretty-printed JSON. The format is text on purpose: it diffs, it
//! survives a partial recovery by hand, and the schema is small enough that the size cost of
//! JSON is irrelevant next to the audio files a session references.
//!
//! # The project folder
//!
//! A document does not sit alone. It lives in a folder of its own, alongside the audio it owns:
//!
//! ```text
//! MySong/
//!   MySong.auris
//!   Audio/
//!     kick.wav
//! ```
//!
//! The folder is what the user moves, copies, renames and archives, and it works because the
//! document refers to everything in it *relatively*. That in turn only holds while **one folder
//! holds one project** — two documents sharing a folder would share its `Audio/` directory, and
//! saving one under a new name would silently leave both pointing at the same files. Which is why
//! [`document_in_folder`] creates the folder rather than trusting anyone to.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use auris_core::Project;
use serde::Deserialize;

use crate::error::{IoError, Result};

/// Extension used for Auris Studio project files, without the leading dot.
pub const PROJECT_EXTENSION: &str = "auris";

/// Sub-folder of a project folder holding the audio that project owns.
pub const AUDIO_DIR: &str = "Audio";

/// The folder a document lives in, which is what its relative asset paths resolve against.
///
/// `None` only for a bare file name with no directory part at all.
pub fn project_folder(document: &Path) -> Option<&Path> {
    document
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// Where a document should be written, given the path a save dialog returned.
///
/// Choosing `/songs/MySong.auris` gives `/songs/MySong/MySong.auris`: saving under a new name
/// creates the folder that name is going to need. Choosing a path whose parent is *already*
/// named after it — which is what saving over an existing project looks like — leaves it where
/// it is rather than burrowing one level deeper each time.
///
/// The extension is *appended* when it is missing, never substituted: `with_extension` would
/// replace a final dot-suffix, so `Mix.v2` — which a Windows save dialog passes through
/// verbatim, `v2` counting as an extension — would quietly become `Mix`, and the save would
/// land on a different project's document, or on the previous version of this one.
pub fn document_in_folder(chosen: &Path) -> PathBuf {
    let already_named = chosen
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PROJECT_EXTENSION));
    let document = if already_named {
        chosen.to_path_buf()
    } else {
        let mut name = chosen.as_os_str().to_os_string();
        name.push(".");
        name.push(PROJECT_EXTENSION);
        PathBuf::from(name)
    };
    let Some(stem) = document.file_stem().map(OsString::from) else {
        return document;
    };
    match project_folder(&document) {
        Some(parent) if parent.file_name() == Some(stem.as_os_str()) => document,
        Some(parent) => parent
            .join(&stem)
            .join(document.file_name().unwrap_or_default()),
        None => PathBuf::from(&stem).join(document.file_name().unwrap_or_default()),
    }
}

/// Just enough of the schema to read the version before committing to a full parse.
#[derive(Deserialize)]
struct FormatVersionProbe {
    #[serde(default = "assumed_format_version")]
    format_version: u32,
}

/// Files written before the field existed are treated as the current version, matching the
/// `serde` default on [`Project`] itself.
fn assumed_format_version() -> u32 {
    Project::FORMAT_VERSION
}

/// Scratch path used while a save is in flight, alongside the file being written.
///
/// It has to be a sibling rather than something under the system temp directory: the final step
/// is a rename, and a rename is only atomic — often only *possible* — within one filesystem.
///
/// Shared with the WAV exporter, which writes the same way for the same reason.
pub(crate) fn in_progress_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("project"));
    name.push(format!(".{}.saving", std::process::id()));
    path.with_file_name(name)
}

/// Writes `project` to `path` as pretty-printed JSON.
///
/// The document is written to a sibling scratch file and then renamed over the target, so an
/// interrupted save — a full disk, a lost connection to a network share — leaves the previous
/// version of the project intact. Writing straight to `path` would truncate it first, and a
/// failure after that point would destroy the user's work with no backup to fall back on, since
/// undo history lives in the application rather than on disk.
pub fn save_project(path: &Path, project: &Project) -> Result<()> {
    let json = serde_json::to_string_pretty(project)?;

    let in_progress = in_progress_path(path);
    // Errors are reported against `path`, not the scratch file, because that is the name the
    // user asked to save and the scratch file is an implementation detail.
    std::fs::write(&in_progress, json).map_err(|e| IoError::from_fs(path, e))?;
    if let Err(error) = std::fs::rename(&in_progress, path) {
        let _ = std::fs::remove_file(&in_progress);
        return Err(IoError::from_fs(path, error));
    }
    Ok(())
}

/// Reads a project from `path`.
///
/// A file written by a newer build is rejected before parsing, so the user gets "update Auris
/// Studio" rather than a confusing message about an unknown field. After a successful parse the
/// id counter is repaired, which is what stops freshly created tracks and clips from colliding
/// with ids already in the document.
pub fn load_project(path: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(path).map_err(|e| IoError::from_fs(path, e))?;

    let probe: FormatVersionProbe = serde_json::from_str(&text)?;
    if probe.format_version > Project::FORMAT_VERSION {
        return Err(IoError::ProjectVersionMismatch {
            found: probe.format_version,
            supported: Project::FORMAT_VERSION,
        });
    }

    let mut project: Project = serde_json::from_str(&text)?;
    project.repair_id_counter();
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;
    use auris_core::{AssetPath, Note, Ticks};

    fn demo_project() -> Project {
        let mut project = Project::new("Demo", 48_000.0);
        project.set_bpm(128.0);
        let lead = project.add_instrument_track("Lead", "auris.synth.pulse");
        let clip = project
            .add_midi_clip(lead, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
        midi.notes
            .push(Note::new(67, Ticks::QUARTER, Ticks::QUARTER));

        let drums = project.add_audio_track("Drums");
        let source = project.add_audio_source(
            "loop",
            AssetPath::inside("Audio/loop.wav"),
            96_000,
            48_000.0,
            2,
        );
        project.add_soundfont("GM", AssetPath::external("/libraries/GM.sf2"), 148_345_812);
        project.add_audio_clip(drums, source, Ticks::ZERO).unwrap();
        project.add_effect(Some(drums), "auris.fx.gain").unwrap();
        project.add_effect(None, "auris.fx.limiter").unwrap();
        project
    }

    #[test]
    fn the_extension_has_no_leading_dot() {
        assert_eq!(PROJECT_EXTENSION, "auris");
        assert!(!PROJECT_EXTENSION.starts_with('.'));
    }

    #[test]
    fn saving_under_a_new_name_creates_the_folder_that_name_needs() {
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong.auris")),
            PathBuf::from("/songs/MySong/MySong.auris")
        );
    }

    #[test]
    fn saving_over_a_project_leaves_it_where_it_is() {
        // Otherwise every save would bury the document one directory deeper than the last.
        let settled = Path::new("/songs/MySong/MySong.auris");
        assert_eq!(document_in_folder(settled), PathBuf::from(settled));
        assert_eq!(
            document_in_folder(&document_in_folder(Path::new("/songs/MySong.auris"))),
            PathBuf::from("/songs/MySong/MySong.auris"),
            "applying the rule twice must reach the same place as applying it once"
        );
    }

    #[test]
    fn a_name_typed_without_an_extension_still_makes_a_project() {
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong")),
            PathBuf::from("/songs/MySong/MySong.auris")
        );
    }

    #[test]
    fn a_dotted_name_keeps_its_dots() {
        // `with_extension` would replace `.v2`, collapsing `Mix.v2` onto `Mix` — a different
        // project, or the previous version of this one, silently saved over. The Windows save
        // dialog passes such a name through verbatim, since `v2` counts as an extension.
        assert_eq!(
            document_in_folder(Path::new("/songs/Mix.v2")),
            PathBuf::from("/songs/Mix.v2/Mix.v2.auris")
        );
        assert_eq!(
            document_in_folder(Path::new("/songs/Mix.v2.auris")),
            PathBuf::from("/songs/Mix.v2/Mix.v2.auris")
        );
    }

    #[test]
    fn the_extension_is_recognised_whatever_its_case() {
        // A document renamed to `.AURIS` elsewhere is still this application's file, not a
        // name to hang another `.auris` off the end of.
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong.AURIS")),
            PathBuf::from("/songs/MySong/MySong.AURIS")
        );
    }

    #[test]
    fn the_folder_of_a_document_is_what_its_assets_resolve_against() {
        assert_eq!(
            project_folder(Path::new("/songs/MySong/MySong.auris")),
            Some(Path::new("/songs/MySong"))
        );
        assert_eq!(project_folder(Path::new("MySong.auris")), None);
    }

    #[test]
    fn a_project_round_trips_through_a_file() {
        let file = TempFile::new("round-trip.auris");
        let project = demo_project();
        save_project(file.path(), &project).unwrap();

        let loaded = load_project(file.path()).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(loaded.bpm(), 128.0);
        assert_eq!(loaded.tracks.len(), 2);
        assert_eq!(loaded.audio_sources.len(), 1);
        assert_eq!(loaded.master.effects.len(), 1);
    }

    #[test]
    fn the_saved_file_is_pretty_printed_json() {
        let file = TempFile::new("pretty.auris");
        save_project(file.path(), &demo_project()).unwrap();
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(text.lines().count() > 20, "file was written on one line");
        assert!(text.contains("\n  \"name\": \"Demo\""));
    }

    #[test]
    fn ids_handed_out_after_loading_do_not_collide() {
        let file = TempFile::new("ids.auris");
        let project = demo_project();
        save_project(file.path(), &project).unwrap();

        let mut loaded = load_project(file.path()).unwrap();
        let mut used: Vec<u64> = Vec::new();
        for track in &loaded.tracks {
            used.push(track.id.0);
            for slot in &track.mixer.effects {
                used.push(slot.id.0);
            }
            match &track.kind {
                auris_core::TrackKind::Instrument(inner) => {
                    used.extend(inner.clips.iter().map(|clip| clip.id.0))
                }
                auris_core::TrackKind::Audio(inner) => {
                    used.extend(inner.clips.iter().map(|clip| clip.id.0))
                }
            }
        }
        used.extend(loaded.master.effects.iter().map(|slot| slot.id.0));
        used.extend(loaded.audio_sources.keys().map(|id| id.0));
        used.extend(loaded.soundfonts.keys().map(|id| id.0));
        // Two tracks, one MIDI clip, one audio source, one SoundFont, one audio clip and two
        // effect slots.
        assert_eq!(used.len(), 8, "demo project should hand out 8 ids");

        let fresh: Vec<u64> = vec![
            loaded.add_audio_track("New").0,
            loaded.next_clip_id().0,
            loaded.next_effect_slot_id().0,
        ];
        for id in &fresh {
            assert!(!used.contains(id), "id {id} was reused after loading");
        }
        assert_eq!(fresh[1], fresh[0] + 1);
        assert_eq!(fresh[2], fresh[1] + 1);
    }

    #[test]
    fn saving_replaces_an_existing_file_and_leaves_no_scratch_behind() {
        let file = TempFile::new("overwrite.auris");
        let mut project = demo_project();
        save_project(file.path(), &project).unwrap();

        project.name = "Renamed".into();
        project.set_bpm(90.0);
        save_project(file.path(), &project).unwrap();

        let loaded = load_project(file.path()).unwrap();
        assert_eq!(loaded.name, "Renamed");
        assert_eq!(loaded.bpm(), 90.0);
        assert_eq!(loaded, project);

        // The atomic-save scratch file must not survive a successful write.
        assert!(!in_progress_path(file.path()).exists());
    }

    #[test]
    fn a_failed_save_leaves_the_previous_version_intact() {
        let file = TempFile::new("preserved.auris");
        let project = demo_project();
        save_project(file.path(), &project).unwrap();
        let before = std::fs::read_to_string(file.path()).unwrap();

        // A directory in place of the scratch file makes the write fail after the point where a
        // truncating save would already have destroyed the target.
        let blocker = in_progress_path(file.path());
        std::fs::create_dir(&blocker).unwrap();
        let mut doomed = demo_project();
        doomed.name = "Should not land".into();
        assert!(save_project(file.path(), &doomed).is_err());
        std::fs::remove_dir(&blocker).unwrap();

        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), before);
        assert_eq!(load_project(file.path()).unwrap(), project);
    }

    #[test]
    fn saving_into_a_missing_directory_reports_the_target_path() {
        let path = std::env::temp_dir()
            .join("auris-io-no-such-directory")
            .join("project.auris");
        match save_project(&path, &demo_project()) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_newer_format_version_is_rejected() {
        let file = TempFile::new("future.auris");
        let mut project = demo_project();
        project.format_version = Project::FORMAT_VERSION + 7;
        save_project(file.path(), &project).unwrap();

        match load_project(file.path()) {
            Err(IoError::ProjectVersionMismatch { found, supported }) => {
                assert_eq!(found, Project::FORMAT_VERSION + 7);
                assert_eq!(supported, Project::FORMAT_VERSION);
            }
            other => panic!("expected a version mismatch, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_reports_a_json_error() {
        let file = TempFile::new("broken.auris");
        std::fs::write(file.path(), "{ not json").unwrap();
        assert!(matches!(load_project(file.path()), Err(IoError::Json(_))));
    }

    #[test]
    fn a_missing_project_reports_file_not_found() {
        let path = std::env::temp_dir().join("auris-io-definitely-missing.auris");
        match load_project(&path) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }
}
