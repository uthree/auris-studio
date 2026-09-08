//! Automatic and manual density must describe the same value the composer receives.

use super::{PartDial, SongDials, song_dials, song_spec};
use crate::harness::{click, drag, open, paint};
use auris_session::prelude::*;
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};

#[test]
fn automatic_density_reads_the_mood_while_manual_density_stays_fixed() {
    let mut part = PartSpec::of_role("lead", Role::Melody);
    let calm = Mood::named("calm").unwrap();
    let driving = Mood::named("driving").unwrap();
    assert!((PartDial::Density.fraction(&part, calm) - 0.225).abs() < 1e-6);
    assert!((PartDial::Density.fraction(&part, driving) - 0.6).abs() < 1e-6);
    assert_eq!(PartDial::Density.text(&part, driving), "60%");
    PartDial::Density.set(&mut part, 0.37);
    assert_eq!(PartDial::Density.fraction(&part, calm), 0.37);
    assert_eq!(PartDial::Density.fraction(&part, driving), 0.37);
}

#[gpui::test]
fn a_density_drag_starts_at_the_mood_and_can_return_to_automatic(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        let mut dials = SongDials::default();
        dials.sections.truncate(1);
        dials.form = vec![dials.sections[0].name.clone()];
        dials.parts = vec![PartSpec::of_role("lead", Role::Melody)];
        dials.mood = Mood::named("calm").unwrap();
        this.song_sheet = Some(dials);
        this.song_advanced = true;
    });
    paint(&app, cx);
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-10000.0))),
        ..Default::default()
    });
    paint(&app, cx);
    let slider = cx.debug_bounds("song-part-density-0").unwrap();
    assert!(slider.top() >= body.top() && slider.bottom() <= body.bottom());
    drag(
        cx,
        slider.center(),
        slider.center() + point(px(1.0), px(0.0)),
    );
    let manual = app.read_with(cx, |this, _| {
        let density = this.song_sheet.as_ref().unwrap().parts[0].density.unwrap();
        let expected = 0.225 + 1.0 / crate::ui::widgets::DRAG_RANGE_PIXELS;
        assert!((density - expected).abs() < 1e-5);
        density
    });
    app.update(cx, |this, _| {
        this.song_sheet.as_mut().unwrap().mood = Mood::named("driving").unwrap();
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(
            PartDial::Density.fraction(&dials.parts[0], dials.mood),
            manual
        );
    });
    click("song-part-density-auto-0", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.parts[0].density, None);
        assert!((PartDial::Density.fraction(&dials.parts[0], dials.mood) - 0.6).abs() < 1e-6);
        let saved = SongSpec::parse(&song_spec(dials).to_toml()).unwrap();
        assert_eq!(song_dials(&saved).parts[0].density, None);
    });
    // Pinning the current automatic value must not change the density either.
    click("song-part-density-auto-0", cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.parts[0].density, Some(dials.mood.density()));
    });
}
