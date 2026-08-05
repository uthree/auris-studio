//! What the window does with files dragged onto it from the desktop.
//!
//! A drop is read by extension rather than by where it landed: a SoundFont is a SoundFont whether
//! it is let go over the lanes or over the mixer, and making a user aim at the right panel to have
//! a file understood would be a rule they had to learn before anything worked. Where it landed
//! decides one thing only — *when* on the timeline dropped audio starts.
//!
//! Two kinds of thing can arrive, and they are not the same kind of thing. Audio and fonts go
//! *into* the open document. A project **is** a document, so opening one closes what is open —
//! which is why [`drop_action`] insists it arrive alone rather than reading a mixed drop as a
//! guess about which document the user meant to keep.
//!
//! The rules live here as free functions so they can be asserted without a window.

use std::path::{Path, PathBuf};

use auris_i18n::{Language, messages};
use gpui::{Bounds, Pixels, Point};

/// What the application can do with one dropped file.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DropKind {
    /// Decode it and put it on a new audio track.
    Audio,
    /// Add it to the project's library of SoundFonts.
    SoundFont,
    /// Open it, in place of the document that is open.
    Project,
    /// Read it as a new document, in place of the one that is open.
    ///
    /// A MIDI file carries its own tempo and meter, so it makes a document rather than joining
    /// one: its notes dropped into a piece running at a different speed would be the right notes
    /// at the wrong lengths, with nothing on screen to say why.
    Midi,
}

/// Dropped paths, split into the ones an importer recognises and the ones none does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Files to import, in the order they were dropped, each with what to do with it.
    pub accepted: Vec<(PathBuf, DropKind)>,
    /// Files no importer here recognises.
    pub rejected: Vec<PathBuf>,
}

/// Sorts dropped paths by extension.
///
/// Purely lexical. The extension is what the file dialogs already filter on, and being sure of a
/// file instead would mean reading the header of every path in the drop — including the
/// hundred-megabyte fonts — before the user has been told that anything is happening at all.
pub fn sort_dropped(paths: &[PathBuf]) -> Dropped {
    let mut dropped = Dropped::default();
    for path in paths {
        match drop_kind(path) {
            Some(kind) => dropped.accepted.push((path.clone(), kind)),
            None => dropped.rejected.push(path.clone()),
        }
    }
    dropped
}

/// What one path is, by its extension.
fn drop_kind(path: &Path) -> Option<DropKind> {
    // Lowercased because a file manager will happily hand over `KICK.WAV`, and the tables are
    // written the way an extension is usually typed.
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let listed = |extensions: &[&str]| extensions.contains(&extension.as_str());
    if extension == auris_session::PROJECT_EXTENSION {
        Some(DropKind::Project)
    } else if listed(auris_session::midi_extensions()) {
        Some(DropKind::Midi)
    } else if listed(auris_session::supported_soundfont_extensions()) {
        Some(DropKind::SoundFont)
    } else if listed(auris_session::supported_audio_extensions()) {
        Some(DropKind::Audio)
    } else {
        None
    }
}

impl DropKind {
    /// Whether this kind of file *becomes* the document rather than joining it.
    ///
    /// The two that do are the two that carry a whole piece's clock. Everything else is material
    /// to put into whatever is already open.
    pub fn replaces_the_document(self) -> bool {
        matches!(self, DropKind::Project | DropKind::Midi)
    }
}

/// What a drop is asking the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// Open this project, in place of the document that is open.
    Open(PathBuf),
    /// Read this MIDI file as a new document, in place of the one that is open.
    OpenMidi(PathBuf),
    /// Read what can be read into the open document, and say what could not.
    Import(Dropped),
    /// Refuse the whole drop, because a project did not arrive alone.
    Confused,
}

impl DropAction {
    /// Whether the drop is going to do anything at all.
    ///
    /// What the border promises while the file is still in the air. It asks the same function the
    /// landing does, so the promise and the act cannot come apart.
    pub fn takes_anything(&self) -> bool {
        match self {
            Self::Open(_) | Self::OpenMidi(_) => true,
            Self::Import(dropped) => !dropped.accepted.is_empty(),
            Self::Confused => false,
        }
    }
}

/// What to do with a set of dropped paths.
///
/// A project — or a MIDI file, which brings a whole piece's clock with it — is a document rather
/// than something that goes *into* one: opening it closes what is open. So it has to arrive on its
/// own. A drop holding one and three takes is not a drop anyone made on purpose, and there is no
/// reading of it that does not risk the work on screen — import into a document about to be
/// replaced, or replace a document the takes were meant for. Two of them have the same problem and
/// no tie-break at all.
pub fn drop_action(paths: &[PathBuf]) -> DropAction {
    let dropped = sort_dropped(paths);
    let replacements: Vec<(&PathBuf, DropKind)> = dropped
        .accepted
        .iter()
        .filter(|(_, kind)| kind.replaces_the_document())
        .map(|(path, kind)| (path, *kind))
        .collect();
    match replacements.as_slice() {
        [] => DropAction::Import(dropped),
        // Alone means alone, refusals included: one of these dragged with a stray file is a hand
        // that grabbed more than it meant to, and this is the one drop that cannot be taken back.
        [(path, kind)] if paths.len() == 1 => match kind {
            DropKind::Midi => DropAction::OpenMidi((*path).clone()),
            _ => DropAction::Open((*path).clone()),
        },
        _ => DropAction::Confused,
    }
}

/// How far along the clip lanes a drop at `pointer` landed, from their left edge.
///
/// `None` when it landed anywhere else — a panel, the transport, the track headers — where there
/// is no position under the pointer to read and the playhead is the answer instead.
pub fn lanes_offset(pointer: Point<Pixels>, lanes: Option<Bounds<Pixels>>) -> Option<Pixels> {
    let lanes = lanes?;
    lanes.contains(&pointer).then(|| pointer.x - lanes.origin.x)
}

/// What became of the files a drop brought.
///
/// Lines rather than counts, because a drop of one file can say something far more useful than a
/// number: which file arrived, how many sounds a font turned out to hold, or why the one thing
/// that was dropped could not be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropOutcome {
    /// The confirmation each imported file produced, in the order they arrived.
    pub imported: Vec<String>,
    /// Why each file that did not arrive did not, refusals and read errors alike.
    pub failed: Vec<String>,
}

impl DropOutcome {
    /// The status line to end on, and whether it reads as a success.
    ///
    /// `None` for a drop that brought nothing at all, which has nothing to say and must not blank
    /// a status line that is still reporting the last thing the user did.
    pub fn summary(&self, language: Language) -> Option<(String, bool)> {
        match (self.imported.as_slice(), self.failed.as_slice()) {
            ([], []) => None,
            // One of anything speaks for itself, and its own line is better than any count: the
            // file's name on the way in, the reason on the way out.
            ([only], []) => Some((only.clone(), true)),
            ([], [only]) => Some((only.clone(), false)),
            (imported, []) => Some((messages::imported_files(language, imported.len()), true)),
            ([], failed) => Some((messages::imported_nothing(language, failed.len()), false)),
            // Partly through. Shown as a failure on purpose: what went in is visible on the
            // timeline and in the library, and what did not is only ever going to be visible here.
            (imported, failed) => Some((
                messages::imported_files_partly(language, imported.len(), failed.len()),
                false,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn audio_and_a_font_are_told_apart() {
        let dropped = sort_dropped(&paths(&["/music/kick.wav", "/fonts/piano.sf2"]));
        assert_eq!(
            dropped.accepted,
            vec![
                (PathBuf::from("/music/kick.wav"), DropKind::Audio),
                (PathBuf::from("/fonts/piano.sf2"), DropKind::SoundFont),
            ]
        );
        assert!(dropped.rejected.is_empty());
    }

    #[test]
    fn the_extension_is_read_whatever_case_it_is_typed_in() {
        let dropped = sort_dropped(&paths(&["KICK.WAV", "Piano.SF2"]));
        assert_eq!(
            dropped
                .accepted
                .iter()
                .map(|(_, kind)| *kind)
                .collect::<Vec<_>>(),
            vec![DropKind::Audio, DropKind::SoundFont]
        );
    }

    #[test]
    fn everything_else_is_rejected_rather_than_guessed_at() {
        let dropped = sort_dropped(&paths(&["notes.txt", "cover.png", "README"]));
        assert!(dropped.accepted.is_empty());
        assert_eq!(dropped.rejected.len(), 3);
    }

    #[test]
    fn a_project_on_its_own_is_opened() {
        assert_eq!(
            drop_action(&paths(&["/songs/MySong/MySong.auris"])),
            DropAction::Open(PathBuf::from("/songs/MySong/MySong.auris"))
        );
    }

    #[test]
    fn a_project_that_did_not_arrive_alone_is_refused_whole() {
        // Every one of these has a reading, and none of them has a reading worth guessing at
        // when getting it wrong closes a document with unsaved work in it.
        for names in [
            vec!["MySong.auris", "kick.wav"],
            vec!["MySong.auris", "notes.txt"],
            vec!["MySong.auris", "Other.auris"],
            vec!["kick.wav", "MySong.auris", "piano.sf2"],
        ] {
            assert_eq!(
                drop_action(&paths(&names)),
                DropAction::Confused,
                "{names:?}"
            );
        }
    }

    #[test]
    fn a_drop_with_no_project_in_it_is_an_import() {
        let action = drop_action(&paths(&["kick.wav", "piano.sf2", "notes.txt"]));
        let DropAction::Import(dropped) = action else {
            panic!("nothing here replaces the document");
        };
        assert_eq!(dropped.accepted.len(), 2);
        assert_eq!(dropped.rejected.len(), 1);
    }

    #[test]
    fn the_border_lights_up_for_exactly_what_the_drop_would_take() {
        // A border that lit up for a drag nothing would take promises something that never
        // happens; one that stayed dark for a readable file hides the gesture the same way it was
        // hidden before any of this existed. Both halves ask `drop_action`, so what this pins is
        // the table itself.
        for (names, lit) in [
            (vec!["kick.wav"], true),
            (vec!["piano.sf2"], true),
            (vec!["MySong.auris"], true),
            (vec!["notes.txt", "take.aiff"], true),
            (vec!["MySong.auris", "take.aiff"], false),
            (vec!["notes.txt"], false),
            (vec![], false),
        ] {
            assert_eq!(
                drop_action(&paths(&names)).takes_anything(),
                lit,
                "{names:?}"
            );
        }
    }

    #[test]
    fn the_order_the_files_were_dropped_in_survives() {
        // The order files are imported in is the order the tracks end up in, so a drop of a
        // numbered set of takes has to keep the numbering the user dragged.
        let dropped = sort_dropped(&paths(&["3.wav", "1.wav", "2.wav"]));
        let names: Vec<_> = dropped
            .accepted
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["3.wav", "1.wav", "2.wav"]);
    }

    fn lanes() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(200.0), px(120.0)),
            size: size(px(800.0), px(400.0)),
        }
    }

    #[test]
    fn a_drop_on_the_lanes_is_measured_from_their_left_edge() {
        let offset = lanes_offset(point(px(260.0), px(300.0)), Some(lanes()));
        assert_eq!(offset, Some(px(60.0)));
    }

    #[test]
    fn a_drop_anywhere_else_has_no_position_to_read() {
        // The library, at the left; and the transport, above.
        assert_eq!(
            lanes_offset(point(px(80.0), px(300.0)), Some(lanes())),
            None
        );
        assert_eq!(
            lanes_offset(point(px(260.0), px(40.0)), Some(lanes())),
            None
        );
        // And the frame before the lanes have ever been painted.
        assert_eq!(lanes_offset(point(px(260.0), px(300.0)), None), None);
    }

    #[test]
    fn one_file_keeps_its_own_line() {
        let outcome = DropOutcome {
            imported: vec!["Imported kick.wav".into()],
            failed: Vec::new(),
        };
        assert_eq!(
            outcome.summary(Language::English),
            Some(("Imported kick.wav".to_string(), true))
        );
    }

    #[test]
    fn one_failure_keeps_its_reason() {
        let outcome = DropOutcome {
            imported: Vec::new(),
            failed: vec!["notes.txt cannot be read".into()],
        };
        let (line, ok) = outcome.summary(Language::English).expect("a line");
        assert_eq!(line, "notes.txt cannot be read");
        assert!(!ok, "nothing arrived");
    }

    #[test]
    fn several_files_are_counted_rather_than_listed() {
        let outcome = DropOutcome {
            imported: vec!["a".into(), "b".into(), "c".into()],
            failed: Vec::new(),
        };
        let (line, ok) = outcome.summary(Language::English).expect("a line");
        assert!(line.contains('3'), "{line}");
        assert!(ok);
    }

    #[test]
    fn a_drop_that_only_partly_arrived_says_both_numbers_and_reads_as_a_failure() {
        let outcome = DropOutcome {
            imported: vec!["a".into(), "b".into()],
            failed: vec!["c".into()],
        };
        let (line, ok) = outcome.summary(Language::English).expect("a line");
        assert!(line.contains('2') && line.contains('1'), "{line}");
        assert!(!ok, "a file that did not arrive is only ever visible here");
    }

    #[test]
    fn a_drop_that_brought_nothing_says_nothing() {
        assert_eq!(DropOutcome::default().summary(Language::English), None);
    }

    #[test]
    fn every_summary_is_written_in_the_language_it_was_asked_for() {
        let outcome = DropOutcome {
            imported: vec!["a".into(), "b".into()],
            failed: vec!["c".into()],
        };
        let english = outcome.summary(Language::English).expect("a line");
        let japanese = outcome.summary(Language::Japanese).expect("a line");
        assert_ne!(english.0, japanese.0);
    }
}
