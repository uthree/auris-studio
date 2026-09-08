//! Song tempo controls exercised through the rendered sheet and its input handler.

use gpui::{Modifiers, TestAppContext, point, px};

use super::{SongDial, song_spec};
use crate::harness::{click, drag_with, open, paint};
use crate::ui::prompt::PromptTarget;

#[gpui::test]
fn song_tempo_accepts_exact_and_fractional_numbers_without_changing_the_document(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);
    let document_tempo = app.read_with(cx, |this, _| this.project().tempo_map.initial_bpm());

    for bpm in [121.0, 174.5, 20.0, 400.0] {
        click("song-tempo-value", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.prompt.as_ref().and_then(|prompt| prompt.target()),
                Some(PromptTarget::SongTempo)
            );
        });
        cx.simulate_input(&bpm.to_string());
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            let dials = this.song_sheet.as_ref().expect("the song sheet stays open");
            assert_eq!(dials.tempo, bpm);
            assert_eq!(song_spec(dials).tempo, bpm);
            assert_eq!(SongDial::Tempo.text(dials), bpm.to_string());
            assert!(this.prompt.is_none());
            assert_eq!(this.project().tempo_map.initial_bpm(), document_tempo);
        });
        paint(&app, cx);
    }
}

#[gpui::test]
fn song_tempo_refuses_invalid_numbers_and_keeps_the_answer_editable(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        this.open_song_sheet();
        this.song_sheet.as_mut().unwrap().tempo = 121.5;
    });
    paint(&app, cx);

    for text in ["0", "19.9", "400.1", "NaN", "inf", "fast"] {
        click("song-tempo-value", cx);
        paint(&app, cx);
        cx.simulate_input(text);
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_sheet.as_ref().unwrap().tempo, 121.5);
            assert!(this.status_failed, "{text} must report why it was refused");
            let prompt = this.prompt.as_ref().expect("the answer remains editable");
            assert_eq!(prompt.field().unwrap().content(), text);
        });
        cx.simulate_keystrokes("escape");
        paint(&app, cx);
    }
}

#[gpui::test]
fn song_tempo_buttons_step_one_beat_and_stop_at_the_range_ends(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        this.open_song_sheet();
        this.song_sheet.as_mut().unwrap().tempo = 120.5;
    });
    paint(&app, cx);
    click("song-tempo-increase", cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.song_sheet.as_ref().unwrap().tempo, 121.5);
    });
    paint(&app, cx);
    click("song-tempo-decrease", cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.song_sheet.as_ref().unwrap().tempo, 120.5);
    });

    for (bpm, selector) in [
        (20.0, "song-tempo-decrease"),
        (400.0, "song-tempo-increase"),
    ] {
        app.update(cx, |this, _| {
            this.song_sheet.as_mut().unwrap().tempo = bpm;
        });
        paint(&app, cx);
        click(selector, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_sheet.as_ref().unwrap().tempo, bpm);
        });
    }
}

#[gpui::test]
fn song_tempo_shift_drag_can_reach_the_beat_a_normal_pixel_skips(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());

    for (distance, shift, expected) in [(1.0, false, 122.0), (3.0, true, 121.0)] {
        app.update(cx, |this, _| {
            this.song_sheet.as_mut().unwrap().tempo = 120.0;
        });
        paint(&app, cx);
        let bounds = cx
            .debug_bounds("song-tempo-dial")
            .expect("tempo is visible");
        let from = bounds.center();
        drag_with(
            cx,
            from,
            point(from.x + px(distance), from.y),
            Modifiers {
                shift,
                ..Modifiers::none()
            },
        );
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_sheet.as_ref().unwrap().tempo, expected);
            assert!(
                this.drag.is_none(),
                "releasing the pointer ends the gesture"
            );
        });
    }
}
