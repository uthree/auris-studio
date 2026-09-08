use super::*;
use crate::history::Edit;
use crate::session::SessionOptions;
use crate::session::fixtures::Scratch;
use auris_compose::{PartSpec, Role, SongSpec};

fn session() -> Session {
    Session::new(SessionOptions::headless().with_balance(false)).unwrap()
}

fn spec(source: PartSource) -> SongSpec {
    let mut spec = SongSpec::parse("form = 'verse'\n[section.verse]\nbars = 1").unwrap();
    let mut part = PartSpec::of_role("selected", Role::Bass);
    part.source = Some(source);
    spec.parts = vec![part];
    spec
}

fn source(path: &Path, bank: i32, patch: i32) -> PartSource {
    PartSource::SoundFont {
        path: path.to_path_buf(),
        bank,
        patch,
    }
}

#[test]
fn source_identity_resolution_compares_collected_and_absolute_paths_without_loading() {
    let mut session = session();
    let scratch = Scratch::new("song-source-identity");
    session.save_as(&scratch.join("Song.auris")).unwrap();
    let before = session.project().clone();
    let relative = source(Path::new("Audio/not-loaded.sf2"), 257, 301);
    let absolute = source(
        &session
            .project_folder()
            .unwrap()
            .join("Audio/not-loaded.sf2"),
        257,
        301,
    );
    assert_eq!(session.resolve_song_source(&relative).unwrap(), absolute);
    assert_eq!(session.resolve_song_source(&absolute).unwrap(), absolute);
    assert!(
        session
            .resolve_song_source(&source(Path::new("../outside.sf2"), 0, 0))
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert!(!session.is_dirty());
}

fn selected(session: &Session) -> (TrackId, PresetRef) {
    let track = session
        .project()
        .tracks
        .iter()
        .find(|track| track.name == "selected")
        .unwrap();
    (track.id, session.track_preset(track.id).unwrap())
}

#[test]
fn a_picker_import_leaves_the_open_document_and_undo_history_untouched() {
    let scratch = Scratch::new("song-font-preview");
    let path = scratch.soundfont("preview.sf2");
    let mut session = session();
    session.add_default_instrument_track("Keep me").unwrap();
    session.forget_history();
    let before = session.project().clone();
    let dirty = session.is_dirty();
    let (name, presets) =
        session.cache_song_soundfont(&path, crate::read_soundfont(&path).unwrap());
    assert_eq!(name, "Test Font");
    assert_eq!((presets[0].bank, presets[0].patch), (0, 0));
    assert_eq!(session.project(), &before);
    assert_eq!(session.is_dirty(), dirty);
    assert!(session.undo().is_none());
    session
        .compose(&auris_compose::compose(&spec(source(&path, 0, 0))))
        .unwrap();
    assert!(session.soundfont_is_loaded(selected(&session).1.font));
}

#[test]
fn exact_soundfont_bank_and_patch_survive_save_open_and_regeneration() {
    let scratch = Scratch::new("song-exact-font");
    let path = scratch.soundfont("custom.sf2");
    // Move the only preset outside the General MIDI bank/program range. This catches code that
    // folds explicit source identities back through the General MIDI picker or a byte-sized id.
    let mut bytes = std::fs::read(&path).unwrap();
    let header = bytes.windows(4).position(|bytes| bytes == b"phdr").unwrap() + 8;
    bytes[header + 20..header + 22].copy_from_slice(&301u16.to_le_bytes());
    bytes[header + 22..header + 24].copy_from_slice(&257u16.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    let mut session = session();
    let spec = spec(source(&path, 257, 301));
    let draft = auris_compose::compose(&spec);
    let report = session.compose(&draft).unwrap();
    assert!(
        !report
            .substituted
            .iter()
            .any(|source| source == "General MIDI")
    );
    let (track, preset) = selected(&session);
    assert_eq!((preset.bank, preset.patch), (257, 301));
    assert_eq!(
        session.project().track(track).unwrap().mixer.gain_db,
        draft.tracks[0].gain_db
    );
    assert_eq!(
        session.song_source_for_preset(preset).unwrap(),
        source(&path, 257, 301)
    );
    let document = session
        .save_as(&scratch.join("Saved.auris"))
        .unwrap()
        .document;
    assert!(session.open(&document).unwrap().is_empty());
    assert_eq!(selected(&session).1, preset);
    let clip = session
        .project()
        .track(track)
        .unwrap()
        .kind
        .as_instrument()
        .unwrap()
        .clips[0]
        .id;
    session.regenerate_clip(clip).unwrap();
    assert_eq!(selected(&session).1, preset);
    let restored = SongSpec::parse(session.project().song_spec.as_ref().unwrap()).unwrap();
    session.compose(&auris_compose::compose(&restored)).unwrap();
    let preset = selected(&session).1;
    assert_eq!(
        session.song_source_for_preset(preset).unwrap(),
        source(&path, 257, 301)
    );
}

#[test]
fn collecting_and_moving_a_song_keeps_its_editable_source_in_the_project() {
    let scratch = Scratch::new("song-collected-font");
    let path = scratch.soundfont("outside.sf2");
    let mut session = session();
    session
        .compose(&auris_compose::compose(&spec(source(&path, 0, 0))))
        .unwrap();
    let first = session
        .save_as(&scratch.join("First.auris"))
        .unwrap()
        .document;
    assert_eq!(session.collect_assets().unwrap(), 1);
    session.save_in_place().unwrap();
    let collected = SongSpec::parse(session.project().song_spec.as_ref().unwrap()).unwrap();
    assert_eq!(
        collected.parts[0].source,
        Some(source(Path::new("Audio/outside.sf2"), 0, 0))
    );
    let copy = session
        .save_as(&scratch.join("Copy.auris"))
        .unwrap()
        .document;
    // Remove both original locations: the new song must be self-contained for re-composition.
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir_all(first.parent().unwrap()).unwrap();
    let moved = scratch.join("Moved");
    std::fs::rename(copy.parent().unwrap(), &moved).unwrap();
    let moved_document = moved.join("Copy.auris");
    assert!(session.open(&moved_document).unwrap().is_empty());
    let restored = SongSpec::parse(session.project().song_spec.as_ref().unwrap()).unwrap();
    session.compose(&auris_compose::compose(&restored)).unwrap();
    assert!(session.soundfont_is_loaded(selected(&session).1.font));
    assert_eq!(session.project().soundfonts.len(), 1);
    assert!(
        session
            .project()
            .soundfonts
            .values()
            .next()
            .unwrap()
            .path
            .is_inside()
    );
}

#[test]
fn a_collected_library_preset_keeps_ownership_when_chosen_for_a_new_song() {
    let scratch = Scratch::new("song-owned-font");
    let path = scratch.soundfont("owned.sf2");
    let mut session = session();
    let font = session.import_soundfont(&path).unwrap();
    session.save_as(&scratch.join("Old.auris")).unwrap();
    session.collect_assets().unwrap();
    let choice = session
        .song_source_for_preset(PresetRef {
            font,
            bank: 0,
            patch: 0,
        })
        .unwrap();
    session
        .compose(&auris_compose::compose(&spec(choice)))
        .unwrap();
    assert!(
        session
            .project()
            .soundfonts
            .values()
            .next()
            .unwrap()
            .path
            .is_inside()
    );
    let restored = SongSpec::parse(session.project().song_spec.as_ref().unwrap()).unwrap();
    assert_eq!(
        restored.parts[0].source,
        Some(source(Path::new("Audio/owned.sf2"), 0, 0))
    );
}

#[test]
fn missing_or_invalid_explicit_sources_never_replace_the_existing_song() {
    let scratch = Scratch::new("song-refused-source");
    let valid = scratch.soundfont("valid.sf2");
    let invalid = scratch.join("invalid.sf2");
    std::fs::write(&invalid, b"not a soundfont").unwrap();
    let sources = [
        source(&scratch.join("missing.sf2"), 0, 0),
        source(&invalid, 0, 0),
        source(&valid, 0, 1),
        PartSource::Clap {
            path: scratch.join("missing.clap"),
            plugin_id: "missing".into(),
        },
        PartSource::Vst3 {
            path: scratch.join("missing.vst3"),
            class_id: "00000000000000000000000000000000".into(),
        },
    ];
    let mut session = session();
    session.add_default_instrument_track("Keep me").unwrap();
    session.forget_history();
    for source in sources {
        let before = session.project().clone();
        let dirty = session.is_dirty();
        let mut spec = spec(source);
        // A valid source first must not alter the live bank before a later source is refused.
        let mut first = PartSpec::of_role("valid first", Role::Melody);
        first.source = Some(self::source(&valid, 0, 0));
        spec.parts.insert(0, first);
        assert!(session.compose(&auris_compose::compose(&spec)).is_err());
        assert_eq!(session.project(), &before);
        assert_eq!(session.is_dirty(), dirty);
        assert!(session.undo().is_none());
    }
}

#[test]
fn composing_and_undoing_restore_the_font_identity_even_when_ids_collide() {
    let scratch = Scratch::new("song-font-undo");
    let original = scratch.soundfont("original.sf2");
    let replacement = scratch.soundfont("replacement.sf2");
    let mut session = session();
    session
        .compose(&auris_compose::compose(&spec(source(&original, 0, 0))))
        .unwrap();
    session.forget_history();
    let before = session.project().clone();
    session
        .compose(&auris_compose::compose(&spec(source(&replacement, 0, 0))))
        .unwrap();
    assert_eq!(session.undo(), Some(Edit::Compose));
    assert_eq!(session.project(), &before);
    assert_eq!(
        session
            .song_source_for_preset(selected(&session).1)
            .unwrap(),
        source(&original, 0, 0)
    );
    assert_eq!(session.redo(), Some(Edit::Compose));
    assert_eq!(
        session
            .song_source_for_preset(selected(&session).1)
            .unwrap(),
        source(&replacement, 0, 0)
    );
}
