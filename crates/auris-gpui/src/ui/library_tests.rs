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
        assert_eq!(this.instrument_rows(cx).len(), expanded_rows);
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}
