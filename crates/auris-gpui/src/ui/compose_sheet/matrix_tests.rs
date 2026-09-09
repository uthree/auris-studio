//! Matrix gestures edit composition participation without touching the open document.

use auris_session::prelude::*;
use gpui::{
    Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px, size,
};

use crate::app::AurisApp;
use crate::harness::{click, open, paint, resize};

use super::super::dials::{SongDials, part_plays_in, song_dials, song_spec};

fn fill_matrix(app: &Entity<AurisApp>, cx: &mut VisualTestContext) -> SongDials {
    let dials = app.update(cx, |this, _| {
        this.open_song_sheet();
        this.song_advanced = true;
        let dials = this.song_sheet.as_mut().unwrap();
        dials.parts = vec![
            PartSpec::of_role("lead", Role::Melody),
            PartSpec::of_role("echo", Role::Melody),
            PartSpec::of_role("kick", Role::Kick),
            PartSpec::of_role("hat", Role::Hat),
        ];
        dials.sections = ["verse", "chorus"]
            .into_iter()
            .map(SectionSpec::named)
            .collect();
        dials.form = vec!["verse".into(), "chorus".into(), "verse".into()];
        dials.clone()
    });
    paint(app, cx);
    reveal_matrix(app, cx);
    dials
}

fn reveal_matrix(app: &Entity<AurisApp>, cx: &mut VisualTestContext) {
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let timeline = cx.debug_bounds("song-matrix-timeline").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), body.top() + px(8.0) - timeline.top())),
        ..Default::default()
    });
    paint(app, cx);
    let cell = cx.debug_bounds("song-matrix-cell-0-0").unwrap();
    assert!(cell.top() >= body.top() && cell.bottom() <= body.bottom());
}

#[gpui::test]
fn a_cell_changes_only_its_part_and_section_even_when_sources_are_shared(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(900.0), px(650.0)));
    let before = fill_matrix(&app, cx);
    assert_eq!(before.parts[0].instrument, before.parts[1].instrument);
    assert_eq!(before.parts[2].instrument, before.parts[3].instrument);

    click("song-matrix-cell-0-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        let mut expected = before.clone();
        expected.sections[0].parts = vec!["echo".into(), "kick".into(), "hat".into()];
        assert_eq!(dials, &expected);
    });

    click("song-matrix-cell-2-1", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        let mut expected = before.clone();
        expected.sections[0].parts = vec!["echo".into(), "kick".into(), "hat".into()];
        expected.sections[1].parts = vec!["lead".into(), "echo".into(), "hat".into()];
        assert_eq!(
            dials, &expected,
            "sharing a kit does not share participation"
        );
    });
}

#[gpui::test]
fn repeated_columns_edit_the_same_section_and_survive_specification_roundtrip(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(900.0), px(650.0)));
    let before = fill_matrix(&app, cx);

    click("song-matrix-cell-0-2", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert!(!part_plays_in(&dials.sections[0], "lead"));
        assert_eq!(dials.sections[1], before.sections[1]);
        let reopened = song_dials(&SongSpec::parse(&song_spec(dials).to_toml()).unwrap());
        assert_eq!(song_spec(&reopened), song_spec(dials));
        assert_eq!(reopened.form, ["verse", "chorus", "verse"]);
    });

    click("song-matrix-cell-0-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.song_sheet.as_ref(),
            Some(&before),
            "the first occurrence sees the repeated occurrence's edit"
        );
    });
}

#[gpui::test]
fn the_last_playing_cell_cannot_turn_an_explicit_roster_back_into_everyone(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(900.0), px(650.0)));
    fill_matrix(&app, cx);
    app.update(cx, |this, _| {
        this.song_sheet.as_mut().unwrap().sections[0].parts = vec!["lead".into()];
    });
    paint(&app, cx);

    click("song-matrix-cell-0-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.song_sheet.as_ref().unwrap().sections[0].parts,
            ["lead"]
        );
    });

    click("song-matrix-cell-1-0", cx);
    paint(&app, cx);
    click("song-matrix-cell-0-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.song_sheet.as_ref().unwrap().sections[0].parts,
            ["echo"]
        );
    });
}

#[gpui::test]
fn a_narrow_matrix_scrolls_to_the_final_section_with_labels_and_footer_fixed(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(640.0), px(480.0)));
    fill_matrix(&app, cx);
    app.update(cx, |this, _| {
        let dials = this.song_sheet.as_mut().unwrap();
        dials.sections = (0..8)
            .map(|index| SectionSpec::named(format!("section {index}")))
            .collect();
        dials.form = dials
            .sections
            .iter()
            .map(|section| section.name.clone())
            .collect();
    });
    paint(&app, cx);
    reveal_matrix(&app, cx);
    let labels = cx.debug_bounds("song-matrix-labels").unwrap();
    let write = cx.debug_bounds("song-sheet-write").unwrap();
    let cancel = cx.debug_bounds("song-sheet-cancel").unwrap();
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let timeline = cx.debug_bounds("song-matrix-timeline").unwrap();
    let first = cx.debug_bounds("song-matrix-cell-0-0").unwrap();
    let last = cx.debug_bounds("song-matrix-cell-0-7").unwrap();
    assert!(
        last.left() >= timeline.right(),
        "later occurrences start outside the visible timeline"
    );

    cx.simulate_event(ScrollWheelEvent {
        position: first.center(),
        delta: ScrollDelta::Pixels(point(px(-10000.0), px(0.0))),
        ..Default::default()
    });
    paint(&app, cx);
    let last = cx.debug_bounds("song-matrix-cell-0-7").unwrap();
    assert!(last.left() >= timeline.left() && last.right() <= timeline.right());
    assert!(last.top() >= body.top() && last.bottom() <= body.bottom());
    assert_eq!(cx.debug_bounds("song-matrix-labels").unwrap(), labels);
    assert_eq!(cx.debug_bounds("song-sheet-write").unwrap(), write);
    assert_eq!(cx.debug_bounds("song-sheet-cancel").unwrap(), cancel);
    assert!(write.top() >= body.bottom() && write.bottom() <= px(480.0));
    assert!(cancel.top() >= body.bottom() && cancel.bottom() <= px(480.0));

    click("song-matrix-cell-0-7", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let sections = &this.song_sheet.as_ref().unwrap().sections;
        assert!(sections[..7].iter().all(|section| section.parts.is_empty()));
        assert_eq!(sections[7].parts, ["echo", "kick", "hat"]);
    });

    let before_vertical = cx.debug_bounds("song-matrix-cell-0-7").unwrap();
    // Scroll back toward the beginning: a wrong vertical-to-horizontal conversion would
    // move away from the rightmost column rather than being hidden by its scroll limit.
    cx.simulate_event(ScrollWheelEvent {
        position: before_vertical.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(64.0))),
        ..Default::default()
    });
    paint(&app, cx);
    let after_vertical = cx.debug_bounds("song-matrix-cell-0-7").unwrap();
    assert!(
        after_vertical.top() > before_vertical.top(),
        "vertical scrolling over the matrix must move the sheet body"
    );
    assert_eq!(after_vertical.left(), before_vertical.left());
    assert_eq!(after_vertical.right(), before_vertical.right());
    assert_eq!(cx.debug_bounds("song-sheet-write").unwrap(), write);
    assert_eq!(cx.debug_bounds("song-sheet-cancel").unwrap(), cancel);
}

#[gpui::test]
fn the_row_source_picker_targets_its_part_without_changing_the_document(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(900.0), px(650.0)));
    let before = fill_matrix(&app, cx);
    let document = app.read_with(cx, |this, _| this.project().clone());
    click("song-matrix-instrument-1", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.project(), &document);
        let chooser = this
            .song_library
            .as_ref()
            .expect("the row opens its library");
        assert_eq!(chooser.part, "echo");
        assert!(chooser.focused);
        assert!(this.menu.is_none());
    });
    // Search and click the shared library's actual result; two pitched rows using the
    // same old instrument remain separate destinations.
    cx.simulate_input("FM");
    paint(&app, cx);
    click("song-lib-auris.synth.fm2", cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        let mut expected = before.clone();
        expected.parts[1].instrument = "auris.synth.fm2".into();
        expected.parts[1].program = None;
        expected.parts[1].source = None;
        assert_eq!(dials, &expected);
        assert!(this.song_library.is_none());
        assert_eq!(this.project(), &document);
    });
}
