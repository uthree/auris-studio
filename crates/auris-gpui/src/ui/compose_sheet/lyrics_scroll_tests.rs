//! Keyboard focus must reveal its editor once, after its height has been laid out.

use auris_session::prelude::SectionSpec;
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px, size};

use crate::app::AurisApp;
use crate::harness::{open, paint, resize};

fn fill_sections(app: &gpui::Entity<AurisApp>, cx: &mut VisualTestContext, advanced: bool) {
    app.update(cx, |this, _| {
        this.open_song_sheet();
        this.song_advanced = advanced;
        let dials = this.song_sheet.as_mut().unwrap();
        dials.sections = ["verse", "chorus", "bridge", "outro"]
            .into_iter()
            .map(|name| {
                let mut section = SectionSpec::named(name);
                section.lyrics = "さくらさいた\nはるがきた\nひかりとどく\nそらをみあげた".into();
                section
            })
            .collect();
        dials.form = dials.sections.iter().map(|s| s.name.clone()).collect();
        // Repeated sections are a single keyboard stop, as they are a single lyrics editor.
        dials.form.push("verse".into());
        this.focus_section_lyrics(0);
    });
    paint(app, cx);
}

fn editor_bounds(cx: &mut VisualTestContext, index: usize) -> gpui::Bounds<gpui::Pixels> {
    let selector: &'static str = Box::leak(format!("song-lyrics-editor-{index}").into_boxed_str());
    cx.debug_bounds(selector)
        .expect("the focused section has an editor")
}

fn assert_visible(app: &gpui::Entity<AurisApp>, cx: &mut VisualTestContext, index: usize) {
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let editor = editor_bounds(cx, index);
    assert!(
        editor.top() >= body.top() && editor.bottom() <= body.bottom(),
        "the focused editor must be inside the scrolling body: editor={editor:?}, body={body:?}"
    );
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.lyrics_edit.as_ref().unwrap().section,
            this.song_sheet.as_ref().unwrap().sections[index].name
        );
    });
}

#[gpui::test]
fn tab_and_shift_tab_reveal_the_new_lyrics_editor_including_wraparound(cx: &mut TestAppContext) {
    for viewport in [size(px(900.0), px(650.0)), size(px(640.0), px(480.0))] {
        for advanced in [false, true] {
            let (app, cx) = open(cx);
            resize(&app, cx, viewport);
            fill_sections(&app, cx, advanced);
            assert_visible(&app, cx, 0);
            for (key, index) in [
                ("tab", 1),
                ("tab", 2),
                ("tab", 3),
                ("tab", 0),
                ("shift-tab", 3),
                ("shift-tab", 2),
            ] {
                cx.simulate_keystrokes(key);
                paint(&app, cx);
                assert_visible(&app, cx, index);
            }
        }
    }
}

#[gpui::test]
fn focusing_a_long_lyrics_editor_reveals_its_end_in_a_small_window(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(640.0), px(480.0)));
    fill_sections(&app, cx, true);
    app.update(cx, |this, _| {
        this.song_sheet.as_mut().unwrap().sections[1].lyrics = "さくらさいた\n".repeat(20);
    });
    paint(&app, cx);
    cx.simulate_keystrokes("tab");
    paint(&app, cx);
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let editor = editor_bounds(cx, 1);
    assert!(editor.bottom() <= body.bottom() && editor.bottom() > body.top());
    app.read_with(cx, |this, _| {
        let edit = this.lyrics_edit.as_ref().unwrap();
        assert_eq!(edit.section, "chorus");
        assert_eq!(edit.field.selection().end, edit.field.content().len());
    });
}

#[gpui::test]
fn manual_scrolling_after_a_focus_change_is_not_pulled_back_to_the_editor(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    resize(&app, cx, size(px(900.0), px(650.0)));
    fill_sections(&app, cx, true);
    for _ in 0..3 {
        cx.simulate_keystrokes("tab");
        paint(&app, cx);
    }
    assert_visible(&app, cx, 3);
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(10000.0))),
        ..Default::default()
    });
    paint(&app, cx);
    let scrolled = editor_bounds(cx, 3);
    assert!(scrolled.bottom() > body.bottom(), "the user scrolled away");
    paint(&app, cx);
    assert_eq!(editor_bounds(cx, 3), scrolled);
    app.read_with(cx, |this, _| {
        assert_eq!(this.lyrics_edit.as_ref().unwrap().section, "outro");
    });
}
