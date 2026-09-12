//! Beginner choices exercised through the menus a person actually opens.

use auris_session::prelude::*;
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};

use super::song_spec;
use crate::harness::{choose, click, open, paint};
use crate::ui::context_menu::MenuCommand;

#[gpui::test]
fn dropdown_fields_have_disclosure_indicators_while_commands_do_not(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);

    assert!(cx.debug_bounds("song-style-dropdown-indicator").is_some());
    assert!(cx.debug_bounds("song-title-dropdown-indicator").is_none());
}

#[gpui::test]
fn sound_choices_generate_modal_songs_and_survive_mood_and_style_changes(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);
    for language in [
        auris_i18n::Language::English,
        auris_i18n::Language::Japanese,
    ] {
        app.update(cx, |this, _| this.language = language);
        for (sound, scale) in [
            (ScaleChoice::Dorian, ScaleId::Dorian),
            (ScaleChoice::Lydian, ScaleId::Lydian),
            (ScaleChoice::Mixolydian, ScaleId::Mixolydian),
            (ScaleChoice::Phrygian, ScaleId::Phrygian),
        ] {
            for (picker, command) in [
                ("song-sound", MenuCommand::SongSound(sound)),
                ("song-mood", MenuCommand::SongMood("dark")),
                ("song-style", MenuCommand::SongPreset("pop-band")),
            ] {
                click(picker, cx);
                paint(&app, cx);
                choose(&app, cx, &command);
                paint(&app, cx);
            }
            app.read_with(cx, |this, _| {
                assert!(!this.song_advanced);
                let dials = this.song_sheet.as_ref().unwrap();
                assert_eq!(dials.sound, Some(sound));
                let spec = song_spec(dials);
                assert_eq!(spec.key.scale, scale);
                assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
                let piece = compose(&spec);
                assert_eq!(piece.harmony.keys.initial().scale, scale);
                assert!(piece.note_count() > 100);
            });
        }
    }
    // Changing the broader tonality resets the optional sound, so the last choice is heard.
    click("song-tonality", cx);
    paint(&app, cx);
    choose(&app, cx, &MenuCommand::SongTonality(Tonality::Major));
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.key.scale, ScaleId::Major);
        assert_eq!(dials.sound, Some(ScaleChoice::Auto));
    });
    for (picker, command) in [
        ("song-sound", MenuCommand::SongSound(ScaleChoice::Auto)),
        ("song-mood", MenuCommand::SongMood("dreamy")),
    ] {
        click(picker, cx);
        paint(&app, cx);
        choose(&app, cx, &command);
        paint(&app, cx);
    }
    app.read_with(cx, |this, _| {
        assert_eq!(this.song_sheet.as_ref().unwrap().key.scale, ScaleId::Lydian)
    });
    // The create gesture writes the selected mode to the document and its saved request.
    click("song-sheet-write", cx);
    app.read_with(cx, |this, _| {
        assert!(this.song_sheet.is_none());
        assert_eq!(this.project().harmony.keys.initial().scale, ScaleId::Lydian);
        let saved = SongSpec::parse(this.project().song_spec.as_deref().unwrap()).unwrap();
        assert_eq!(saved.key.scale, ScaleId::Lydian);
    });
}

#[gpui::test]
fn an_exact_key_replaces_the_sound_choice(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| this.open_song_sheet());
    paint(&app, cx);
    click("song-sound", cx);
    paint(&app, cx);
    choose(&app, cx, &MenuCommand::SongSound(ScaleChoice::Dorian));
    paint(&app, cx);
    click("song-advanced", cx);
    paint(&app, cx);
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let key = cx.debug_bounds("song-key").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), body.center().y - key.center().y)),
        ..Default::default()
    });
    paint(&app, cx);
    click("song-key", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(this.prompt.is_some()));
    cx.simulate_input("Eb harmonic-minor");
    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(10000.0))),
        ..Default::default()
    });
    paint(&app, cx);
    click("song-mood", cx);
    paint(&app, cx);
    choose(&app, cx, &MenuCommand::SongMood("bright"));
    app.read_with(cx, |this, _| {
        let dials = this.song_sheet.as_ref().unwrap();
        assert_eq!(dials.key.to_text(), "Eb harmonic-minor");
        assert_eq!(dials.tonality, None);
        assert_eq!(dials.sound, None);
    });
}

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
