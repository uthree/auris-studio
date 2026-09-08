//! Part role choices exercised through the detailed settings and their rendered picker.

use auris_session::prelude::*;
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};

use crate::harness::{choose, click, open, paint};
use crate::ui::context_menu::{MenuCommand, MenuEntry};

#[gpui::test]
fn the_part_role_picker_preserves_adjustments_when_reselecting_or_changing_roles(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let before = app.update(cx, |this, _| {
        this.open_song_sheet();
        let dials = this.song_sheet.as_mut().unwrap();
        dials.parts = vec![PartSpec {
            instrument: "custom.instrument".to_string(),
            program: Some(gm::Program(48)),
            octave: 6,
            density: Some(0.67),
            subdivision: Subdivision::EighthTriplet,
            gate: 0.42,
            rhythm: Some(Pattern::parse("X..x..x.").unwrap()),
            gain_db: -4.5,
            pan: -0.6,
            ..PartSpec::of_role("my lead", Role::Melody)
        }];
        for section in &mut dials.sections {
            section.parts = vec!["my lead".to_string()];
            section.tweaks.clear();
        }
        dials.clone()
    });
    paint(&app, cx);
    click("song-advanced", cx);
    paint(&app, cx);

    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let picker = cx.debug_bounds("song-part-role-0").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), body.center().y - picker.center().y)),
        ..Default::default()
    });
    paint(&app, cx);

    for (current, next) in [
        (Role::Melody, Role::Melody),
        (Role::Melody, Role::Bass),
        (Role::Bass, Role::Stab),
    ] {
        click("song-part-role-0", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let checked: Vec<_> = this
                .menu
                .as_ref()
                .expect("clicking the role opens its picker")
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    MenuEntry::Item(item) if item.checked => Some(item.command.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                checked,
                vec![MenuCommand::SongPartRole {
                    part: 0,
                    role: current,
                }]
            );
        });
        choose(
            &app,
            cx,
            &MenuCommand::SongPartRole {
                part: 0,
                role: next,
            },
        );
        app.read_with(cx, |this, _| {
            let mut expected = before.clone();
            expected.parts[0].role = next;
            assert_eq!(this.song_sheet.as_ref(), Some(&expected));
            assert!(this.menu.is_none(), "choosing the role closes its picker");
        });
        paint(&app, cx);
    }
}
