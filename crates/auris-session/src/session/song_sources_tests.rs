use super::*;
use crate::session::fixtures::Scratch;
use crate::session::{SessionOptions, SongIdea, SongRequest, SongSourceChoice};

fn session() -> Session {
    Session::new(SessionOptions::headless()).unwrap()
}

/// A note-free song document whose one part, `selected`, plays `source`.
fn authored(source: PartSource) -> Result<Session, String> {
    SongRequest {
        sources: vec![SongSourceChoice {
            role: "selected".into(),
            instrument: "auris.synth.fm2".into(),
            source: Some(source),
        }],
        ..SongRequest::default()
    }
    .authoring_session(&SongIdea {
        title: "Draft".into(),
        description: "Bright keys".into(),
        tempo: 120.0,
        key: "C major".into(),
        density: 0.5,
        lead: "piano".into(),
    })
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
    let (_, name, presets) = session
        .cache_song_soundfont(&path, crate::read_soundfont(&path).unwrap())
        .unwrap();
    assert_eq!(name, "Test Font");
    assert_eq!((presets[0].bank, presets[0].patch), (0, 0));
    assert_eq!(session.project(), &before);
    assert_eq!(session.is_dirty(), dirty);
    assert!(session.undo().is_none());
    let song = authored(source(&path, 0, 0)).unwrap();
    assert!(song.soundfont_is_loaded(selected(&song).1.font));
}

#[test]
fn exact_soundfont_bank_and_patch_survive_save_and_open() {
    let scratch = Scratch::new("song-exact-font");
    let path = scratch.soundfont("custom.sf2");
    // Move the only preset outside the General MIDI bank/program range. This catches code that
    // folds explicit source identities back through the General MIDI picker or a byte-sized id.
    let mut bytes = std::fs::read(&path).unwrap();
    let header = bytes.windows(4).position(|bytes| bytes == b"phdr").unwrap() + 8;
    bytes[header + 20..header + 22].copy_from_slice(&301u16.to_le_bytes());
    bytes[header + 22..header + 24].copy_from_slice(&257u16.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    let mut session = authored(source(&path, 257, 301)).unwrap();
    let (_, preset) = selected(&session);
    assert_eq!((preset.bank, preset.patch), (257, 301));
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
}

#[test]
fn missing_or_invalid_explicit_sources_are_refused() {
    let scratch = Scratch::new("song-refused-source");
    let invalid = scratch.join("invalid.sf2");
    std::fs::write(&invalid, b"not a soundfont").unwrap();
    let valid = scratch.soundfont("valid.sf2");
    for source in [
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
    ] {
        assert!(authored(source.clone()).is_err(), "{source:?} was accepted");
    }
}
