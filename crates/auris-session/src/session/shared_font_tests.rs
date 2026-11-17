//! Shared library behavior across documents, history branches and application launches.

use std::path::Path;
use std::sync::Arc;

use super::fixtures::{Scratch, session};
use super::{Session, SessionOptions};

fn with_library(path: &Path) -> Session {
    Session::new(SessionOptions::headless().with_soundfont_library(path)).unwrap()
}

#[test]
fn imports_survive_new_projects_and_restarts_without_becoming_document_edits() {
    let scratch = Scratch::new("shared-font-restart");
    let library = scratch.join("soundfonts.json");
    let path = scratch.soundfont("Piano.sf2");
    let mut first = with_library(&library);
    first.forget_history();
    let id = first.import_soundfont(&path).unwrap();
    assert!(!first.is_dirty());
    assert!(!first.can_undo());
    // An alternative spelling of an existing path must not allocate a second library entry.
    let alias = path.parent().unwrap().join(".").join("Piano.sf2");
    assert_eq!(first.import_soundfont(&alias).unwrap(), id);
    assert_eq!(crate::library::soundfonts::read(&library).unwrap().len(), 1);
    let samples = first.fonts.get(id).unwrap();
    first.new_project();
    let font = first.soundfonts().next().unwrap().id;
    assert_eq!(first.soundfonts().count(), 1);
    assert!(Arc::ptr_eq(&first.fonts.get(font).unwrap(), &samples));
    assert!(!first.is_dirty());
    drop(first);

    let second = with_library(&library);
    let font = second.soundfonts().next().unwrap().id;
    assert_eq!(second.soundfont_presets(font)[0].name, "Test Piano");
    assert!(!second.is_dirty());
    assert!(!second.can_undo());
}

#[test]
fn adding_a_library_font_after_undo_keeps_both_history_branches_playable() {
    let scratch = Scratch::new("shared-font-history");
    let mut session = with_library(&scratch.join("soundfonts.json"));
    session.forget_history();
    let a = session.add_default_instrument_track("A").unwrap();
    let b = session.add_default_instrument_track("B").unwrap();
    session.undo().unwrap();
    let font = session
        .import_soundfont(&scratch.soundfont("Piano.sf2"))
        .unwrap();
    assert!(font.0 > b.0);
    session.undo().unwrap();
    assert!(session.project().track(a).is_none());
    assert!(session.soundfont_is_loaded(font));
    assert!(!session.is_dirty());
    session.redo().unwrap();
    session.redo().unwrap();
    assert!(session.project().track(b).is_some());
    assert!(session.soundfont_is_loaded(font));
}

#[test]
fn opening_an_older_project_keeps_its_original_font_id_and_preset() {
    let scratch = Scratch::new("shared-font-existing");
    let library = scratch.join("soundfonts.json");
    let path = scratch.soundfont("Piano.sf2");
    let mut old = session();
    let font = old.import_soundfont(&path).unwrap();
    let track = old.add_default_instrument_track("Piano").unwrap();
    let preset = auris_core::PresetRef {
        font,
        bank: 0,
        patch: 0,
    };
    old.set_track_preset(track, preset).unwrap();
    old.save_as(&scratch.join("Old.auris")).unwrap();
    let saved = old.path().unwrap().to_path_buf();
    drop(old);

    let mut session = with_library(&library);
    session.import_soundfont(&path).unwrap();
    assert!(session.open(&saved).unwrap().is_empty());
    assert_eq!(session.soundfonts().count(), 1);
    assert_eq!(session.track_preset(track), Some(preset));
    session.add_default_instrument_track("Other").unwrap();
    session.undo().unwrap();
    assert!(session.soundfont_is_loaded(font));
    assert!(!session.is_dirty());
}

#[test]
fn missing_library_files_remain_visible_without_preventing_new_projects() {
    let scratch = Scratch::new("shared-font-missing");
    let library = scratch.join("soundfonts.json");
    let path = scratch.soundfont("Piano.sf2");
    with_library(&library).import_soundfont(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut session = with_library(&library);
    session.new_project();
    assert_eq!(session.soundfonts().count(), 1);
    let font = session.soundfonts().next().unwrap();
    assert_eq!(font.name, "Test Font");
    assert!(!session.soundfont_is_loaded(font.id));
    assert!(!session.is_dirty());
}

#[test]
fn reimporting_a_moved_font_repairs_the_library_without_duplicate_entries() {
    let scratch = Scratch::new("shared-font-moved");
    let library = scratch.join("soundfonts.json");
    let path = scratch.soundfont("Piano.sf2");
    let mut session = with_library(&library);
    let original = session.import_soundfont(&path).unwrap();
    let destination = scratch.join("moved");
    std::fs::create_dir(&destination).unwrap();
    let moved = destination.join("Piano.sf2");
    std::fs::rename(&path, &moved).unwrap();
    assert_eq!(session.import_soundfont(&moved).unwrap(), original);
    assert_eq!(crate::library::soundfonts::read(&library).unwrap().len(), 1);
    drop(session);
    let session = with_library(&library);
    let font = session.soundfonts().next().unwrap();
    assert_eq!(session.soundfonts().count(), 1);
    assert!(session.soundfont_is_loaded(font.id));
}

#[test]
fn a_malformed_catalog_is_preserved_and_a_failed_import_does_not_change_the_document() {
    let scratch = Scratch::new("shared-font-malformed");
    let library = scratch.join("soundfonts.json");
    std::fs::write(&library, b"broken catalog").unwrap();
    let mut session = with_library(&library);
    let before = session.project().clone();
    assert!(
        session
            .import_soundfont(&scratch.soundfont("Piano.sf2"))
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert!(!session.is_dirty());
    assert_eq!(std::fs::read(&library).unwrap(), b"broken catalog");
}

#[test]
fn a_picker_import_is_available_to_tracks_and_later_sessions() {
    let scratch = Scratch::new("shared-font-picker");
    let library = scratch.join("soundfonts.json");
    let path = scratch.soundfont("Piano.sf2");
    let mut session = with_library(&library);
    let (_, name, presets) = session
        .cache_song_soundfont(&path, super::read_soundfont(&path).unwrap())
        .unwrap();
    assert_eq!(name, "Test Font");
    assert_eq!(presets.len(), 1);
    assert_eq!(session.soundfonts().count(), 1);
    assert!(!session.is_dirty());
    drop(session);
    assert_eq!(with_library(&library).soundfonts().count(), 1);
}

#[test]
fn opening_or_starting_a_document_with_library_fonts_builds_the_graph_once() {
    // A second build straight after the first loads every hosted plugin again, because the first
    // graph still holds the instances it was given.
    let scratch = Scratch::new("shared-font-one-build");
    let library = scratch.join("soundfonts.json");
    let mut session = with_library(&library);
    session
        .import_soundfont(&scratch.soundfont("Piano.sf2"))
        .unwrap();
    session.add_default_instrument_track("Piano").unwrap();
    session.save_as(&scratch.join("Song.auris")).unwrap();
    let saved = session.path().unwrap().to_path_buf();

    let before = session.graph_builds;
    session.open(&saved).unwrap();
    assert_eq!(session.soundfonts().count(), 1);
    assert_eq!(session.graph_builds - before, 1, "open");

    let before = session.graph_builds;
    session.new_project();
    assert_eq!(session.soundfonts().count(), 1);
    assert_eq!(session.graph_builds - before, 1, "new project");
}
