//! Library gestures change the intended track and preserve an unavailable search choice.

use super::*;
use crate::harness::{click, open, paint, with_a_clip};
use auris_i18n::Language;
use gpui::TestAppContext;

fn right_click(selector: &'static str, cx: &mut gpui::VisualTestContext) {
    let at = cx
        .debug_bounds(selector)
        .expect("the sound row is visible")
        .center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, gpui::Modifiers::none());
    cx.simulate_mouse_up(at, gpui::MouseButton::Right, gpui::Modifiers::none());
}

fn prepare_library(app: &mut AurisApp) {
    app.panels = crate::dock::PanelLayout::default();
    app.language = Language::Japanese;
    app.library = LibraryTree::default();
    app.leave_library_search();
}

#[gpui::test]
fn overwriting_a_user_sound_confirms_and_uses_the_captured_track(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (original, replacement) = app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
        let original = this
            .session
            .capture_sound(track, "Management overwrite fixture")
            .unwrap();
        this.sounds = auris_session::SoundBook::save_sound(
            &auris_session::SoundBook::path(),
            original.clone(),
        )
        .unwrap();
        this.session
            .apply_sound(track, &auris_session::factory_sounds()[2])
            .unwrap();
        this.session
            .add_effect(Some(track), "auris.fx.reverb")
            .unwrap();
        let replacement = this.session.capture_sound(track, &original.name).unwrap();
        (original, replacement)
    });
    paint(&app, cx);
    click("library-search", cx);
    cx.simulate_input("Management overwrite fixture");
    paint(&app, cx);
    right_click("lib-user-sound:Management overwrite fixture", cx);
    paint(&app, cx);
    click("menu-item-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(this.prompt.as_ref().unwrap().title.contains(&original.name));
        assert!(matches!(
            &this.prompt.as_ref().unwrap().body,
            crate::ui::prompt::PromptBody::Ask(crate::ui::prompt::Question::OverwriteSound {
                expected, replacement: captured, source,
            }) if **expected == original && **captured == replacement && source == "Test"
        ));
    });
    cx.simulate_keystrokes("escape");
    assert!(
        auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&original)
    );
    paint(&app, cx);
    right_click("lib-user-sound:Management overwrite fixture", cx);
    paint(&app, cx);
    // The menu and confirmation also accept the keyboard.
    cx.simulate_keystrokes("down enter");
    let before = app.update(cx, |this, _| {
        assert!(this.prompt.is_some());
        this.selected_track = None;
        this.session
            .apply_sound(track, &auris_session::factory_sounds()[0])
            .unwrap();
        this.session.forget_history();
        this.project().clone()
    });
    paint(&app, cx);
    click("prompt-ok", cx);
    app.update(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(this.sounds.entries().contains(&replacement));
        assert_eq!(*this.project(), before);
        assert!(this.session.undo().is_none());
    });
    assert!(
        auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&replacement)
    );
}

#[gpui::test]
fn user_sounds_can_be_renamed_and_deleted_without_a_selected_track(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (original, before) = app.update(cx, |this, _| {
        prepare_library(this);
        let original = this
            .session
            .capture_sound(track, "Management rename fixture")
            .unwrap();
        this.session.apply_sound(track, &original).unwrap();
        assert!(this.session.sound_is_applied(track, &original));
        auris_session::SoundBook::save_sound(&auris_session::SoundBook::path(), original.clone())
            .unwrap();
        this.sounds = this
            .session
            .save_sound(
                track,
                "Management taken fixture",
                &auris_session::SoundBook::path(),
            )
            .unwrap();
        this.selected_track = None;
        this.session.forget_history();
        (original, this.project().clone())
    });
    paint(&app, cx);
    click("library-search", cx);
    cx.simulate_input("Management rename fixture");
    paint(&app, cx);
    right_click("lib-user-sound:Management rename fixture", cx);
    paint(&app, cx);
    click("menu-item-0", cx);
    app.read_with(cx, |this, _| {
        assert!(this.menu.is_some());
        assert!(this.prompt.is_none());
    });
    // Rename is the first enabled item when there is no source track.
    cx.simulate_keystrokes("down enter");
    paint(&app, cx);
    cx.simulate_input("Management taken fixture");
    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    assert!(cx.debug_bounds("prompt-error").is_some());
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input(" ");
    cx.simulate_keystrokes("enter");
    app.read_with(cx, |this, _| assert!(this.prompt.is_some()));
    cx.simulate_keystrokes("escape");
    assert!(
        auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&original)
    );
    paint(&app, cx);
    right_click("lib-user-sound:Management rename fixture", cx);
    paint(&app, cx);
    click("menu-item-1", cx);
    paint(&app, cx);
    cx.simulate_input("Management renamed fixture");
    cx.simulate_keystrokes("enter");
    let mut renamed = original;
    renamed.name = "Management renamed fixture".into();
    app.read_with(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(this.sounds.entries().contains(&renamed));
        assert_eq!(*this.project(), before);
    });
    paint(&app, cx);
    click("library-search", cx);
    cx.simulate_input("Management renamed fixture");
    paint(&app, cx);
    right_click("lib-user-sound:Management renamed fixture", cx);
    paint(&app, cx);
    click("menu-item-3", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(this.prompt.as_ref().unwrap().title.contains(&renamed.name))
    });
    click("prompt-cancel", cx);
    assert!(
        auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&renamed)
    );
    paint(&app, cx);
    right_click("lib-user-sound:Management renamed fixture", cx);
    paint(&app, cx);
    click("menu-item-3", cx);
    paint(&app, cx);
    click("prompt-ok", cx);
    app.update(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(!this.sounds.entries().contains(&renamed));
        assert!(this.session.sound_is_applied(track, &renamed));
        assert_eq!(*this.project(), before);
        assert!(this.session.undo().is_none());
    });
    assert!(
        !auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&renamed)
    );
}

#[gpui::test]
fn a_stale_user_sound_confirmation_preserves_the_external_edit(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let original = app.update(cx, |this, _| {
        prepare_library(this);
        let original = this
            .session
            .capture_sound(track, "Management stale fixture")
            .unwrap();
        this.sounds = auris_session::SoundBook::save_sound(
            &auris_session::SoundBook::path(),
            original.clone(),
        )
        .unwrap();
        original
    });
    paint(&app, cx);
    click("library-search", cx);
    cx.simulate_input("Management stale fixture");
    paint(&app, cx);
    right_click("lib-user-sound:Management stale fixture", cx);
    paint(&app, cx);
    click("menu-item-3", cx);
    let updated = auris_session::SoundBook::rename_sound(
        &auris_session::SoundBook::path(),
        &original,
        "Management external fixture",
    )
    .unwrap();
    paint(&app, cx);
    click("prompt-ok", cx);
    paint(&app, cx);
    assert!(cx.debug_bounds("prompt-error").is_some());
    app.read_with(cx, |this, _| {
        assert!(this.prompt.is_some());
        // Other tests may save unrelated entries concurrently; compare the target only.
        assert!(!this.sounds.entries().contains(&original));
        let external = updated
            .entries()
            .iter()
            .find(|sound| sound.name == "Management external fixture")
            .unwrap();
        assert!(this.sounds.entries().contains(external));
    });
    cx.simulate_keystrokes("escape");
}

#[gpui::test]
fn duplicating_a_saved_sound_preserves_its_source_and_project_without_a_selected_track(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (original, before) = app.update(cx, |this, _| {
        prepare_library(this);
        this.session
            .apply_sound(track, &auris_session::factory_sounds()[2])
            .unwrap();
        this.session
            .add_effect(Some(track), "auris.fx.reverb")
            .unwrap();
        let original = this
            .session
            .capture_sound(track, "Duplicate fixture")
            .unwrap();
        auris_session::SoundBook::save_sound(&auris_session::SoundBook::path(), original.clone())
            .unwrap();
        let mut existing = original.clone();
        existing.name = "Duplicate fixtureのコピー".into();
        this.sounds =
            auris_session::SoundBook::save_sound(&auris_session::SoundBook::path(), existing)
                .unwrap();
        this.selected_track = None;
        this.session.forget_history();
        (original, this.project().clone())
    });
    paint(&app, cx);
    click("library-search", cx);
    cx.simulate_input("Duplicate fixture");
    paint(&app, cx);
    right_click("lib-user-sound:Duplicate fixture", cx);
    paint(&app, cx);
    click("menu-item-2", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let prompt = this.prompt.as_ref().unwrap();
        assert!(prompt.title.contains(&original.name));
        assert_eq!(
            prompt.field().unwrap().content(),
            "Duplicate fixtureのコピー (2)"
        );
    });
    cx.simulate_keystrokes("escape");
    assert!(
        !auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .iter()
            .any(|sound| sound.name == "Duplicate fixtureのコピー (2)")
    );
    paint(&app, cx);
    right_click("lib-user-sound:Duplicate fixture", cx);
    paint(&app, cx);
    click("menu-item-2", cx);
    paint(&app, cx);
    cx.simulate_input("Duplicate fixture");
    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    assert!(cx.debug_bounds("prompt-error").is_some());
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("Duplicate fixtureのコピー (2)");
    cx.simulate_keystrokes("enter");
    let mut copy = original.clone();
    copy.name = "Duplicate fixtureのコピー (2)".into();
    app.update(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(this.sounds.entries().contains(&original));
        assert!(this.sounds.entries().contains(&copy));
        assert_eq!(*this.project(), before);
        assert!(this.session.undo().is_none());
    });
    assert!(
        auris_session::SoundBook::read(&auris_session::SoundBook::path())
            .unwrap()
            .entries()
            .contains(&copy)
    );
}

#[gpui::test]
fn saving_and_searching_a_user_sound_restores_its_chain_and_supports_undo(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
        this.session
            .apply_sound(track, &auris_session::factory_sounds()[2])
            .unwrap();
        this.session
            .add_effect(Some(track), "auris.fx.reverb")
            .unwrap();
    });
    paint(&app, cx);
    click("save-user-sound", cx);
    paint(&app, cx);
    cx.simulate_input("Window sound fixture");
    cx.simulate_keystrokes("enter");
    app.update(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(
            this.sounds
                .entries()
                .iter()
                .any(|sound| sound.name == "Window sound fixture")
        );
        this.session
            .apply_sound(track, &auris_session::factory_sounds()[0])
            .unwrap();
        this.session.forget_history();
    });
    paint(&app, cx);
    click("library-search", cx);
    paint(&app, cx);
    cx.simulate_input("Window sound fixture");
    paint(&app, cx);
    click("lib-user-sound:Window sound fixture", cx);
    app.update(cx, |this, _| {
        let sound = this
            .sounds
            .entries()
            .iter()
            .find(|sound| sound.name == "Window sound fixture")
            .unwrap();
        assert!(this.session.sound_is_applied(track, sound));
        assert_eq!(this.project().track(track).unwrap().mixer.effects.len(), 1);
        assert_eq!(
            this.session.undo(),
            Some(auris_session::Edit::ApplySoundPreset)
        );
        assert!(
            this.project()
                .track(track)
                .unwrap()
                .mixer
                .effects
                .is_empty()
        );
    });
}

#[gpui::test]
fn strings_presets_are_searchable_in_japanese(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
    });
    paint(&app, cx);
    click("library-search", cx);
    paint(&app, cx);
    cx.simulate_input("チェロ");
    paint(&app, cx);
    click("lib-factory-sound:Cello", cx);
    app.read_with(cx, |this, _| {
        assert!(
            this.session
                .sound_is_applied(track, &auris_session::factory_sounds()[2])
        );
    });
}

#[gpui::test]
fn a_physical_family_opens_its_own_presets_and_applies_an_undoable_derivative(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    let before = app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
        this.project().clone()
    });
    paint(&app, cx);
    assert!(cx.debug_bounds("lib-factory-sound:Pick bass").is_none());
    click("lib-family-auris.physical.bass", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| assert_eq!(*this.project(), before));
    click("lib-factory-sound:Pick bass", cx);
    app.update(cx, |this, _| {
        let sound = auris_session::factory_sounds()
            .into_iter()
            .find(|sound| sound.name == "Pick bass")
            .unwrap();
        assert!(this.session.sound_is_applied(track, &sound));
        assert_eq!(
            this.session.undo(),
            Some(auris_session::Edit::ApplySoundPreset)
        );
        assert_eq!(*this.project(), before);
    });
    click("lib-family-auris.physical.bass", cx);
    app.read_with(cx, |this, _| {
        let (index, _) = auris_session::factory_sound_families()
            .iter()
            .enumerate()
            .find(|(_, family)| family.instrument_id == "auris.physical.bass")
            .unwrap();
        assert!(!this.library.is_open(Branch::InstrumentPresets(index)));
    });
}

#[gpui::test]
fn other_physical_family_presets_are_searchable_in_japanese(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
    });
    for (query, selector, name) in [
        (
            "フェルトピアノ",
            "lib-factory-sound:Felt piano",
            "Felt piano",
        ),
        (
            "ミュートダルシマー",
            "lib-factory-sound:Muted dulcimer",
            "Muted dulcimer",
        ),
        (
            "ドライドラムキット",
            "lib-factory-sound:Dry drum kit",
            "Dry drum kit",
        ),
        (
            "ソプラノ合唱",
            "lib-factory-sound:Soprano choir",
            "Soprano choir",
        ),
    ] {
        paint(&app, cx);
        click("library-search", cx);
        paint(&app, cx);
        cx.simulate_input(query);
        paint(&app, cx);
        click(selector, cx);
        app.read_with(cx, |this, _| {
            let sound = auris_session::factory_sounds()
                .into_iter()
                .find(|sound| sound.name == name)
                .unwrap();
            assert!(this.session.sound_is_applied(track, &sound), "{name}");
        });
    }
}

#[gpui::test]
fn plugin_discovery_error_offers_a_retry_that_clears_the_failed_state(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        prepare_library(this);
        this.clap_files = None;
        this.vst3_files = None;
        this.plugin_discovery_error = Some("temporary worker failure".into());
        // Keep the retry in the viewport. `debug_bounds` also reports clipped descendants,
        // while a pointer gesture at their off-window coordinates correctly hits nothing.
        this.library.set_open(Branch::Instruments, false);
        this.library.set_open(Branch::UserSounds, false);
        this.library.set_open(Branch::SoundFonts, false);
        this.library.set_open(Branch::Voices, false);
        this.library.set_open(Branch::Effects, false);
    });

    paint(&app, cx);
    assert!(cx.debug_bounds("plugin-rescan").is_some());
    click("plugin-rescan", cx);

    app.read_with(cx, |this, _| {
        assert!(this.plugin_discovery_error.is_none());
    });
}

#[gpui::test]
fn plugin_metadata_cache_evicts_before_adding_a_sixty_fifth_file(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        for index in 0..PLUGIN_METADATA_CACHE_LIMIT {
            this.clap_contents.insert(
                std::path::PathBuf::from(format!("plugin-{index}.clap")),
                std::sync::Arc::from(Vec::<auris_session::ClapPluginInfo>::new()),
            );
        }
        assert!(this.reserve_plugin_cache_slot(0));
        this.vst3_contents.insert(
            std::path::PathBuf::from("incoming.vst3"),
            std::sync::Arc::from(Vec::<auris_session::Vst3PluginInfo>::new()),
        );
        assert_eq!(
            this.clap_contents.len() + this.vst3_contents.len(),
            PLUGIN_METADATA_CACHE_LIMIT
        );
    });
}

#[gpui::test]
fn installed_plugin_rows_page_a_large_inventory_without_losing_search_reveal(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        prepare_library(this);
        this.settings.plugin_paths.clear();
        this.clap_files = Some(std::sync::Arc::from(
            (0..(PLUGIN_FILE_PAGE_SIZE * 2 + 1))
                .map(|index| std::path::PathBuf::from(format!("plugin-{index}.clap")))
                .collect::<Vec<_>>(),
        ));
        this.vst3_files = Some(std::sync::Arc::from(Vec::<std::path::PathBuf>::new()));

        let rows = this.installed_plugin_rows(0, cx);
        // Heading, hint, rescan, add-folder, page status and page controls stay in addition to one
        // bounded file page; the remaining inventory is not eagerly materialised as GPUI rows.
        assert_eq!(rows.len(), PLUGIN_FILE_PAGE_SIZE + 6);

        let last = PLUGIN_FILE_PAGE_SIZE * 2;
        this.reveal_library_branch(Branch::PluginFile(last));
        assert_eq!(this.library.plugin_page(last + 1), last..last + 1);
    });
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
            this.instrument_rows(cx).len(),
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
            this.instrument_rows(cx).len(),
            expanded_rows
                - category_members
                - auris_session::factory_sound_families()
                    .iter()
                    .filter(|family| this
                        .registry()
                        .descriptor(&family.instrument_id)
                        .is_some_and(|descriptor| descriptor.category == PluginCategory::Synth))
                    .count()
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
        assert_eq!(this.instrument_rows(cx).len(), expanded_rows);
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}

#[gpui::test]
fn plugin_folder_removal_names_the_folder_and_requires_confirmation(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let folder = std::env::temp_dir().join(format!(
        "auris-plugin-folder-to-remove-{}",
        std::process::id()
    ));
    app.update(cx, |this, _| {
        prepare_library(this);
        this.settings.plugin_paths = vec![folder.clone()];
        // Put the plugin-folder row in the visible viewport. Hit-testing a debug node drawn far
        // below a clipped scroll view would only prove that off-screen controls do not click.
        for branch in [
            Branch::Instruments,
            Branch::UserSounds,
            Branch::SoundFonts,
            Branch::Voices,
            Branch::Effects,
        ] {
            this.library.set_open(branch, false);
        }
    });
    paint(&app, cx);
    click("forget-plugin-path-0", cx);
    app.read_with(cx, |this, _| {
        let prompt = this.prompt.as_ref().expect("removal asks first");
        assert!(prompt.title.contains(&folder.display().to_string()));
        assert!(matches!(
            &prompt.body,
            crate::ui::prompt::PromptBody::Ask(
                crate::ui::prompt::Question::RemovePluginPath(path)
            ) if path == &folder
        ));
        assert_eq!(
            this.settings.plugin_paths.as_slice(),
            std::slice::from_ref(&folder)
        );
    });

    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.settings.plugin_paths.as_slice(),
            std::slice::from_ref(&folder)
        )
    });
}

#[gpui::test]
fn library_shelf_navigation_returns_without_editing_the_track(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let before = app.update(cx, |this, _| {
        prepare_library(this);
        this.select_track(track);
        this.project().clone()
    });
    paint(&app, cx);
    let at = cx.debug_bounds("lib-branch-音源").unwrap().center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_up(at, gpui::MouseButton::Left, gpui::Modifiers::none());
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.library.shelf, Some(Branch::Instruments))
    });
    click("library-root", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.library.shelf, None);
        assert_eq!(this.project(), &before);
    });
}
