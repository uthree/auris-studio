//! Beginner choices exercised through the menus a person actually opens.

use auris_session::prelude::*;
use gpui::TestAppContext;

use super::song_spec;
use crate::harness::{choose, click, open, paint};
use crate::ui::context_menu::MenuCommand;

#[test]
fn imported_key_and_tempo_remain_explicit_when_the_mood_changes() {
    let spec = SongSpec::parse("key = 'D dorian'\ntempo = 97.5").unwrap();
    let mut dials = super::song_dials(&spec);
    super::set_song_mood(&mut dials, Mood::named("dark").unwrap());
    assert_eq!(dials.key, spec.key);
    assert_eq!(dials.tempo, spec.tempo);
    assert_eq!(dials.tonality, None);
    assert_eq!(dials.pace, None);

    dials.pace = Some(Pace::Auto);
    dials.sections[0].tempo = Some(140.0);
    super::set_song_mood(&mut dials, Mood::named("dark").unwrap());
    assert_eq!(dials.tempo, 80.0);
    assert_eq!(
        dials.sections[0].tempo,
        Some(140.0),
        "a section's explicit tempo overrides the mood-derived song tempo"
    );
}

#[gpui::test]
fn a_dark_minor_song_can_be_requested_without_opening_advanced_controls(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);
    assert!(cx.debug_bounds("song-matrix-timeline").is_none());
    for (picker, command) in [
        ("song-mood", MenuCommand::SongMood("dark")),
        ("song-tonality", MenuCommand::SongTonality(Tonality::Minor)),
        ("song-pace", MenuCommand::SongPace(Pace::Slow)),
    ] {
        click(picker, cx);
        paint(&app, cx);
        choose(&app, cx, &command);
        paint(&app, cx);
    }
    app.read_with(cx, |this, _| {
        assert!(!this.song_advanced);
        let spec = song_spec(this.song_sheet.as_ref().unwrap());
        assert!(spec.key.is_minor());
        assert_eq!(spec.tempo, 80.0);
        assert_eq!(spec.mood, Mood::named("dark").unwrap());
        assert!(spec.charts.values().all(|chart| chart.is_unwritten()));
        let saved = SongSpec::parse(&spec.to_toml()).unwrap();
        assert_eq!(saved, spec);
        let piece = compose(&saved);
        assert!(piece.note_count() > 100);
        assert!(piece.harmony.keys.initial().is_minor());
        assert!(
            !this.project().harmony.keys.initial().is_minor(),
            "editing the request must not edit the document"
        );
    });
}

#[gpui::test]
fn mood_defaults_and_explicit_choices_survive_a_style_change(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);
    click("song-mood", cx);
    paint(&app, cx);
    choose(&app, cx, &MenuCommand::SongMood("dark"));
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert!(dials.key.is_minor());
        assert_eq!(dials.tempo, 80.0);
    });
    for (picker, command) in [
        ("song-tonality", MenuCommand::SongTonality(Tonality::Major)),
        ("song-pace", MenuCommand::SongPace(Pace::Fast)),
        ("song-style", MenuCommand::SongPreset("pop-band")),
        ("song-mood", MenuCommand::SongMood("tense")),
    ] {
        click(picker, cx);
        paint(&app, cx);
        choose(&app, cx, &command);
        paint(&app, cx);
    }
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.parts, preset("pop-band").unwrap().spec().parts);
        assert!(!dials.key.is_minor());
        assert_eq!(dials.tempo, 160.0);
        assert_eq!(dials.mood, Mood::named("tense").unwrap());
        assert!(dials.charts.iter().all(|(_, chart)| chart.is_unwritten()));
    });
}
