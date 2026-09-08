//! Library gestures change the intended track and preserve an unavailable search choice.

use super::*;
use crate::harness::{click, open, paint, with_a_clip};
use auris_i18n::Language;
use gpui::TestAppContext;

fn prepare_library(app: &mut AurisApp) {
    app.panels = crate::dock::PanelLayout::default();
    app.language = Language::Japanese;
    app.library = LibraryTree::default();
    app.leave_library_search();
}

#[test]
fn the_song_font_catalog_keeps_one_file_after_adoption_and_retains_available_imports() {
    let adopted_path = std::env::temp_dir().join("adopted.sf2");
    let missing_path = std::env::temp_dir().join("missing.sf2");
    let detached = vec![
        SongLibraryFont {
            id: SoundFontId(100),
            name: "Adopted import".into(),
            path: adopted_path.clone(),
            presets: Vec::new(),
        },
        SongLibraryFont {
            id: SoundFontId(101),
            name: "Available import".into(),
            path: missing_path.clone(),
            presets: Vec::new(),
        },
    ];
    let catalog = song_font_catalog(
        vec![
            (
                SoundFontId(1),
                "Loaded project font".into(),
                Some(adopted_path),
                true,
            ),
            (
                SoundFontId(2),
                "Unavailable project font".into(),
                Some(missing_path),
                false,
            ),
        ],
        &detached,
    );
    assert_eq!(
        catalog,
        vec![
            (SoundFontId(1), "Loaded project font".into()),
            (SoundFontId(101), "Available import".into()),
        ]
    );
}

#[gpui::test]
fn choosing_a_japanese_search_result_changes_only_the_selected_instrument(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let other = app.update(cx, |this, _| {
        prepare_library(this);
        this.session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let other = this.session.add_default_instrument_track("Other").unwrap();
        this.session
            .set_track_instrument(other, "auris.synth.fm2")
            .unwrap();
        this.select_track(track);
        other
    });
    paint(&app, cx);
    click("library-search", cx);
    paint(&app, cx);
    cx.simulate_input("チップチューン");
    paint(&app, cx);
    click("lib-auris.synth.chiptune", cx);

    app.read_with(cx, |this, _| {
        assert_eq!(this.selected_track, Some(track));
        for (id, expected) in [(track, "auris.synth.chiptune"), (other, "auris.synth.fm2")] {
            assert_eq!(
                this.project()
                    .track(id)
                    .unwrap()
                    .kind
                    .as_instrument()
                    .unwrap()
                    .instrument_id,
                expected
            );
        }
        assert!(this.library_search.content().is_empty());
        assert!(!this.library_search_focused);
    });
}

#[gpui::test]
fn an_instrument_result_without_a_compatible_target_keeps_the_query_and_document(
    cx: &mut TestAppContext,
) {
    // An unselected instrument track and a selected audio track are both unavailable targets.
    for select_audio in [false, true] {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            prepare_library(this);
            this.session.add_default_instrument_track("Unused").unwrap();
            if select_audio {
                let audio = this.session.add_audio_track("Audio");
                this.select_track(audio);
            } else {
                this.selected_track = None;
            }
            serde_json::to_value(this.project()).unwrap()
        });
        paint(&app, cx);
        click("library-search", cx);
        paint(&app, cx);
        cx.simulate_input("チップチューン");
        paint(&app, cx);
        click("lib-auris.synth.chiptune", cx);

        app.read_with(cx, |this, _| {
            assert_eq!(this.library_search.content(), "チップチューン");
            assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
        });
    }
}

#[gpui::test]
fn a_category_disclosure_hides_and_restores_its_instrument_rows(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (before, expanded_rows, category_members) = app.update(cx, |this, cx| {
        prepare_library(this);
        this.select_track(track);
        let category_members = this
            .registry()
            .instruments()
            .filter(|plugin| plugin.category == PluginCategory::Synth)
            .count();
        assert!(category_members > 0);
        (
            serde_json::to_value(this.project()).unwrap(),
            this.instrument_rows(LibraryTarget::Track, cx).len(),
            category_members,
        )
    });
    paint(&app, cx);
    assert!(cx.debug_bounds("lib-auris.synth.chiptune").is_some());

    click("lib-branch-シンセ", cx);
    paint(&app, cx);
    // gpui 0.2.2 retains debug_bounds across frames, including selectors no longer drawn.
    // Count the rows produced by the current tree instead of treating that cache as presence.
    app.update(cx, |this, cx| {
        assert!(
            !this
                .library
                .is_open(Branch::InstrumentCategory(PluginCategory::Synth))
        );
        assert_eq!(
            this.instrument_rows(LibraryTarget::Track, cx).len(),
            expanded_rows - category_members
        );
    });

    click("lib-branch-シンセ", cx);
    paint(&app, cx);
    assert!(cx.debug_bounds("lib-auris.synth.chiptune").is_some());
    app.update(cx, |this, cx| {
        assert!(
            this.library
                .is_open(Branch::InstrumentCategory(PluginCategory::Synth))
        );
        assert_eq!(
            this.instrument_rows(LibraryTarget::Track, cx).len(),
            expanded_rows
        );
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}

#[gpui::test]
fn song_library_search_selects_only_the_part_and_preserves_the_document_browser(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (before, original_tree, untouched_parts) = app.update(cx, |this, cx| {
        prepare_library(this);
        this.select_track(track);
        this.library_search = TextField::new("keep this query");
        this.library_search_focused = true;
        this.library.set_open(Branch::Instruments, false);
        this.open_song_sheet();
        let dials = this.song_sheet.as_mut().unwrap();
        dials.parts[0].instrument = "auris.synth.fm2".into();
        dials.parts[0].program = None;
        dials.parts[0].source = None;
        let untouched = dials.parts[1..].to_vec();
        this.open_song_library(0, cx);
        (
            serde_json::to_value(this.project()).unwrap(),
            this.library.clone(),
            untouched,
        )
    });
    paint(&app, cx);
    assert!(
        cx.debug_bounds("song-lib-auris.sampler.soundfont")
            .is_none()
    );
    click("song-library-search", cx);
    paint(&app, cx);
    cx.simulate_input("チップチューン");
    paint(&app, cx);
    click("song-lib-auris.synth.chiptune", cx);

    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.parts[0].instrument, "auris.synth.chiptune");
        assert_eq!(dials.parts[1..], untouched_parts);
        assert!(this.song_library.is_none());
        assert_eq!(this.library_search.content(), "keep this query");
        assert!(this.library_search_focused);
        assert_eq!(this.library, original_tree);
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}

#[gpui::test]
fn song_library_typing_and_ime_never_change_the_covered_lyrics_editor(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;

    let (app, cx) = open(cx);
    let (lyrics, sections) = app.update(cx, |this, cx| {
        prepare_library(this);
        this.open_song_sheet();
        this.song_sheet.as_mut().unwrap().sections[0].lyrics = "歌詞の下書き".into();
        this.focus_section_lyrics(0);
        this.open_song_library(0, cx);
        (
            this.lyrics_edit.clone(),
            this.song_sheet.as_ref().unwrap().sections.clone(),
        )
    });
    paint(&app, cx);
    cx.simulate_input("あいう");
    cx.simulate_keystrokes("backspace");
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.song_library.as_ref().unwrap().search.content(), "あい");
        assert_eq!(this.lyrics_edit, lyrics);
    });
    cx.update(|window, cx| {
        app.update(cx, |this, cx| {
            this.replace_and_mark_text_in_range(None, "サンプル", Some(4..4), window, cx);
        })
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let search = &this.song_library.as_ref().unwrap().search;
        assert_eq!(search.content(), "あいサンプル");
        assert!(search.marked().is_some());
        assert_eq!(this.lyrics_edit, lyrics);
    });
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| assert!(this.song_library.is_some()));
    cx.update(|window, cx| {
        app.update(cx, |this, cx| {
            this.replace_text_in_range(None, "音色", window, cx);
        })
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let search = &this.song_library.as_ref().unwrap().search;
        assert_eq!(search.content(), "あい音色");
        assert!(search.marked().is_none());
    });
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| {
        assert!(this.song_library.is_none());
        assert_eq!(this.lyrics_edit, lyrics);
        assert_eq!(this.song_sheet.as_ref().unwrap().sections, sections);
    });
}

#[gpui::test]
fn song_library_uses_an_imported_font_exact_bank_and_patch_without_editing_the_document(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let path = std::env::temp_dir().join("auris-library-choice.sf2");
    let before = app.update(cx, |this, cx| {
        prepare_library(this);
        this.open_song_sheet();
        this.song_library_fonts.push(SongLibraryFont {
            id: SoundFontId(u64::MAX),
            name: "Custom sound library".into(),
            path: path.clone(),
            presets: vec![SoundFontPreset {
                name: "固有のベル".into(),
                bank: 17,
                patch: 93,
            }],
        });
        this.open_song_library(0, cx);
        assert!(this.taking_text_input());
        serde_json::to_value(this.project()).unwrap()
    });
    paint(&app, cx);
    cx.simulate_input("固有のベル");
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.song_library.as_ref().unwrap().search.content(),
            "固有のベル"
        );
    });
    click("song-lib-preset-18446744073709551615-17-93", cx);

    app.read_with(cx, |this, _| {
        assert_eq!(
            this.song_sheet.as_ref().unwrap().parts[0].source,
            Some(PartSource::SoundFont {
                path: path.clone(),
                bank: 17,
                patch: 93
            })
        );
        assert!(this.song_sheet.as_ref().unwrap().parts[0].program.is_none());
        assert!(this.song_library.is_none());
        assert_eq!(this.song_library_fonts.len(), 1);
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}

#[gpui::test]
fn song_library_selects_cached_hosted_instruments_and_excludes_effects(cx: &mut TestAppContext) {
    for vst3 in [false, true] {
        let (app, cx) = open(cx);
        let file = std::env::temp_dir().join(if vst3 {
            "SongChoice.vst3"
        } else {
            "SongChoice.clap"
        });
        let source = if vst3 {
            PartSource::Vst3 {
                path: file.clone(),
                class_id: "00000000000000000000000000000001".into(),
            }
        } else {
            PartSource::Clap {
                path: file.clone(),
                plugin_id: "song-choice.instrument".into(),
            }
        };
        let before = app.update(cx, |this, cx| {
            prepare_library(this);
            this.clap_files = Some(if vst3 { Vec::new() } else { vec![file.clone()] });
            this.vst3_files = Some(if vst3 { vec![file.clone()] } else { Vec::new() });
            if vst3 {
                this.vst3_contents.insert(
                    file.clone(),
                    [PluginKind::Instrument, PluginKind::Effect]
                        .into_iter()
                        .enumerate()
                        .map(|(index, kind)| auris_session::Vst3PluginInfo {
                            class_id: format!("{:032}", index + 1),
                            name: format!("Hosted {index}"),
                            vendor: "Test".into(),
                            version: "1".into(),
                            kind,
                            category: PluginCategory::Synth,
                            has_gui: false,
                        })
                        .collect(),
                );
            } else {
                this.clap_contents.insert(
                    file.clone(),
                    [PluginKind::Instrument, PluginKind::Effect]
                        .into_iter()
                        .map(|kind| auris_session::ClapPluginInfo {
                            clap_id: format!(
                                "song-choice.{}",
                                if kind == PluginKind::Instrument {
                                    "instrument"
                                } else {
                                    "effect"
                                }
                            ),
                            name: "Hosted".into(),
                            vendor: "Test".into(),
                            description: String::new(),
                            version: "1".into(),
                            kind,
                            category: PluginCategory::Synth,
                        })
                        .collect(),
                );
            }
            this.open_song_sheet();
            this.open_song_library(0, cx);
            let browser = this.song_library.as_mut().unwrap();
            browser.tree.set_open(Branch::PluginFile(0), true);
            // Keep only the plugin branch in view; this is a real row gesture, not a call
            // to the source setter, and the ordinary browser stays independently collapsed.
            browser.tree.set_open(Branch::Instruments, false);
            browser.tree.set_open(Branch::SoundFonts, false);
            this.library.set_open(Branch::Plugins, false);
            let rows = this.installed_plugin_rows(LibraryTarget::SongPart, 0, cx);
            let ordinary = this.installed_plugin_rows(LibraryTarget::Track, 0, cx);
            assert_eq!(ordinary.len(), 1);
            assert_eq!(rows.len(), 5); // heading, hint, folder action, file, instrument.
            serde_json::to_value(this.project()).unwrap()
        });
        paint(&app, cx);
        click(
            if vst3 {
                "song-lib-vst3:00000000000000000000000000000001"
            } else {
                "song-lib-clap:song-choice.instrument"
            },
            cx,
        );
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_sheet.as_ref().unwrap().parts[0].source,
                Some(source)
            );
            assert!(this.song_library.is_none());
            assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
        });
    }
}
