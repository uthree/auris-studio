use super::*;
use crate::{auxiliary_window::Surface, harness};
use gpui::{Entity, VisualTestContext, point, size};

#[test]
fn frame_history_accepts_observed_motion_once_and_stays_bounded() {
    let mut view = PhysicalView::default();
    for index in 0..20 {
        let mut frame = MotionFrame {
            active: 1,
            ..Default::default()
        };
        frame.voices[0].pitch = index as f32;
        frame.voices[0].points[1] = 0.25;
        assert!(view.accept_frame(frame));
    }
    assert_eq!(view.history.len(), PhysicalView::HISTORY_LIMIT);
    assert_eq!(view.history[0].voices[0].pitch, 8.);
    assert!(!view.accept_frame(view.frame.unwrap()));
}

#[test]
fn frozen_view_does_not_accept_new_frames_or_advance_trails() {
    let mut view = PhysicalView::default();
    let first = MotionFrame {
        active: 1,
        ..Default::default()
    };
    assert!(view.accept_frame(first));
    view.frozen = true;
    let mut changed = first;
    changed.voices[0].points[17] = 0.75;
    assert!(!view.accept_frame(changed));
    assert_eq!(view.history.len(), 1);
    assert_eq!(view.frame, Some(first));
}

#[test]
fn a_lane_projection_keeps_fixed_ends_scale_and_every_sample() {
    let mut voice = MotionVoice::default();
    for (index, value) in voice.points.iter_mut().enumerate() {
        *value = (std::f32::consts::PI * index as f32 / 63.).sin() * 0.01;
    }
    voice.points[0] = 0.3;
    let loud = body_points(&voice, MotionGeometry::String, 24.);
    assert_eq!(loud.len(), 64);
    assert_eq!((loud[0], loud[63]), (0., 0.));
    voice.points.iter_mut().for_each(|value| *value *= 0.1);
    let quiet = body_points(&voice, MotionGeometry::String, 24.);
    assert!((quiet[32] / loud[32] - 0.1).abs() < 1e-5);
    // A bore's mouthpiece is not fixed: its pressure is drawn as it is.
    assert!(body_points(&voice, MotionGeometry::Bore, 24.)[0] > 0.);
    assert_eq!(displayed_motion(f32::NAN, 6.), 0.);
    assert_eq!(displayed_motion(1., 6.), 1.);
}

#[test]
fn a_pointer_on_a_lane_is_read_against_the_string_as_drawn() {
    let bounds = Bounds::new(point(px(100.), px(50.)), size(px(220.), px(40.)));
    // The anchors sit LANE_INSET inside the lane.
    assert_eq!(lane_touch(bounds, point(px(110.), px(70.))), (0., 0.));
    assert_eq!(lane_touch(bounds, point(px(310.), px(70.))).0, 1.);
    let (at, pull) = lane_touch(bounds, point(px(160.), px(60.)));
    assert!((at - 0.25).abs() < 1e-6);
    assert!((pull - 0.5).abs() < 1e-6);
    // Off the lane, a pull is as hard as it goes rather than harder.
    assert_eq!(lane_touch(bounds, point(px(0.), px(500.))), (0., -1.));
    assert_eq!(touch_velocity(0.), 0.3);
    assert_eq!(touch_velocity(-1.), 1.);
}

#[test]
fn a_pointer_on_a_head_is_a_radius_and_off_it_is_nothing() {
    // A wide cell fits its head into the square at its centre, as the heatmap draws it.
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
    assert_eq!(head_touch(bounds, point(px(100.), px(50.))), Some(0.));
    let rim = head_touch(bounds, point(px(100.), px(5.))).unwrap();
    assert!((rim - 0.9).abs() < 1e-5);
    assert_eq!(head_touch(bounds, point(px(10.), px(50.))), None);
}

#[test]
fn a_pulled_string_is_two_straight_segments_from_its_anchors() {
    let shape = pulled_string(0.25, 1.);
    assert_eq!(shape.len(), auris_session::MOTION_POINTS);
    assert_eq!(shape[0], 0.);
    assert!(shape[63].abs() < 1e-6);
    let apex = shape
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert!((apex as f32 / 63. - 0.25).abs() < 0.02, "apex at {apex}");
    assert!(pulled_string(0.5, -0.5).iter().all(|value| *value <= 0.));
}

#[test]
fn lanes_keep_a_chord_in_pitch_order_whatever_the_voices_recency() {
    let mut frame = MotionFrame {
        active: 3,
        ..Default::default()
    };
    for (voice, pitch) in frame.voices.iter_mut().zip([60., 67., 64.]) {
        voice.pitch = pitch;
        voice.level = 0.5;
    }
    assert_eq!(lane_voices(&frame), [Some(1), Some(2), Some(0), None]);
    frame.voices.swap(0, 2);
    assert_eq!(lane_voices(&frame), [Some(1), Some(0), Some(2), None]);
}

#[test]
fn every_model_is_played_the_way_the_instrument_is() {
    assert_eq!(Gesture::of("auris.physical.guitar"), Gesture::Pluck);
    assert_eq!(Gesture::of("auris.physical.bass"), Gesture::Pluck);
    assert_eq!(Gesture::of("auris.physical.piano"), Gesture::Key);
    assert_eq!(Gesture::of("auris.physical.violin"), Gesture::Sustain);
    assert_eq!(Gesture::of("auris.physical.clarinet"), Gesture::Sustain);
    assert_eq!(Gesture::of("auris.physical.choir"), Gesture::Sustain);
    assert_eq!(
        Gesture::of("auris.physical.hammered_dulcimer"),
        Gesture::Strike
    );
    assert_eq!(Gesture::of("auris.synth.drumkit"), Gesture::Strike);
    assert_eq!(
        resting_geometry("auris.physical.tin_whistle", MotionGeometry::String),
        MotionGeometry::Bore
    );
    assert_eq!(
        resting_geometry("auris.physical.choir", MotionGeometry::String),
        MotionGeometry::Tract
    );
}

#[test]
fn drum_preview_covers_the_kit() {
    let mut pitch = 36;
    let mut pitches = Vec::new();
    for _ in 0..12 {
        pitches.push(pitch);
        pitch = next_drum_preview(pitch, true, false);
    }
    assert_eq!(pitch, 36);
    assert_eq!(next_drum_preview(36, false, false), 50);
    assert!(pitches.contains(&51) && pitches.contains(&46));
    assert_eq!(drum_name(38, false, Language::Japanese), "スネア");
}

#[test]
fn an_electronic_kit_previews_and_names_its_own_pads() {
    let mut pitch = 36;
    let mut pitches = Vec::new();
    for _ in 0..13 {
        pitches.push(pitch);
        pitch = next_drum_preview(pitch, true, true);
    }
    assert_eq!(pitch, 36);
    for pad in [35, 37, 39, 56] {
        assert!(pitches.contains(&pad), "{pad} is previewed");
    }
    assert_eq!(drum_name(39, true, Language::English), "Clap");
    assert_eq!(drum_name(56, true, Language::Japanese), "カウベル");
    // The modelled kit has no clap of its own: 39 is its snare's neighbour.
    assert_eq!(drum_name(39, false, Language::English), "Snare");
    assert_eq!(Gesture::of("auris.synth.trap_kit"), Gesture::Strike);
    assert_eq!(Gesture::of("auris.synth.boom_bass"), Gesture::Key);
    assert_eq!(
        resting_geometry("auris.synth.edm_kit", MotionGeometry::String),
        MotionGeometry::Oscillator
    );
}

/// Opens `id` on a new track in the plugin window and returns the window to drive.
fn open_model(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    id: &str,
) -> (TrackId, VisualTestContext) {
    app.update(cx, |app, cx| {
        app.close_plugin_window();
        cx.notify();
    });
    harness::paint(app, cx);
    let track = app.update(cx, |app, cx| {
        let track = app.session.add_instrument_track("Model", id).unwrap();
        app.open_plugin_window(PluginSubject::Instrument(track));
        app.poll_physical_view();
        cx.notify();
        track
    });
    harness::paint(app, cx);
    let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
    let mut utility = VisualTestContext::from_window(handle.into(), cx);
    harness::resize(app, &mut utility, size(px(1080.), px(760.)));
    // A window already that size is not laid out again by the resize.
    harness::paint(app, &mut utility);
    (track, utility)
}

#[gpui::test]
fn every_physical_model_shows_its_bodies_in_every_direction(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    for (id, analysis) in [
        ("auris.physical.clarinet", false),
        ("auris.physical.guitar", false),
        ("auris.physical.electric_guitar", false),
        ("auris.physical.bass", false),
        ("auris.physical.violin", false),
        ("auris.physical.piano", true),
        ("auris.physical.bell", true),
        ("auris.physical.mallet", true),
        ("auris.physical.hammered_dulcimer", true),
        ("auris.physical.tin_whistle", false),
        ("auris.physical.choir", true),
        ("auris.synth.drumkit", true),
        ("auris.synth.fm2", true),
        ("auris.synth.chiptune", true),
        ("auris.synth.supersaw", true),
        ("auris.synth.kick_bass", true),
        ("auris.synth.edm_kit", true),
    ] {
        let (track, mut utility) = open_model(&app, cx, id);
        app.read_with(&utility, |app, _| {
            assert!(app.session.has_instrument_motion(track), "{id} has a body");
        });
        assert!(utility.debug_bounds("physical-motion").is_some(), "{id}");
        assert!(utility.debug_bounds("physical-audition").is_some(), "{id}");
        for lane in 0..LANES {
            let selector = Box::leak(format!("physical-body-{lane}").into_boxed_str());
            assert!(utility.debug_bounds(selector).is_some(), "{id} lane {lane}");
        }
        assert_eq!(
            utility.debug_bounds("physical-analysis").is_some(),
            analysis,
            "{id}"
        );
    }
    let (_, mut utility) = open_model(&app, cx, "auris.physical.choir");
    for direction in [
        atelier::Direction::Studio,
        atelier::Direction::Glass,
        atelier::Direction::Mono,
    ] {
        for appearance in [atelier::Appearance::Light, atelier::Appearance::Dark] {
            app.update(&mut utility, |app, cx| {
                app.theme.atelier = atelier::Theme::new(direction, appearance);
                app.theme.install(cx);
                let mut frame = MotionFrame {
                    geometry: MotionGeometry::Tract,
                    active: 1,
                    ..Default::default()
                };
                frame.voices[0].pitch = 60.;
                frame.voices[0].level = 0.5;
                frame.voices[0].held = true;
                frame.voices[0].points[20] = 0.02;
                frame.voices[0].modes = [1., 0.8, 0.5, 0.4, 0.6, 0.9, 0.7, 0.3];
                app.physical_view.accept_frame(frame);
                app.physical_view.frozen = true;
                cx.notify();
            });
            harness::resize(&app, &mut utility, size(px(1080.), px(760.)));
            assert!(utility.debug_bounds("physical-analysis").is_some());
            harness::resize(&app, &mut utility, size(px(520.), px(480.)));
            assert!(utility.debug_bounds("physical-body-3").is_some());
        }
    }
}

#[gpui::test]
fn a_string_is_pulled_aside_and_plucked_where_it_is_let_go(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    let (_, mut utility) = open_model(&app, cx, "auris.physical.guitar");
    let before = app.read_with(&utility, |app, _| app.project().clone());
    let lane = utility.debug_bounds("physical-body-0").unwrap();
    let grab = point(lane.left() + lane.size.width * 0.3, lane.center().y);
    harness::press(&mut utility, grab);
    harness::paint(&app, &mut utility);
    app.read_with(&utility, |app, _| {
        let touch = app.physical_view.touch.expect("the string is held");
        assert!(!touch.sounding, "a pluck sounds when it is let go");
        assert_eq!(touch.pitch, 60);
        assert!((touch.at - 0.3).abs() < 0.05, "held at {}", touch.at);
    });
    // Pull it up, past the top of its lane.
    let pulled = point(grab.x + px(20.), lane.top() - px(40.));
    harness::drag_to(&mut utility, point(grab.x + px(10.), lane.top()));
    harness::paint(&app, &mut utility);
    harness::drag_to(&mut utility, pulled);
    harness::paint(&app, &mut utility);
    app.read_with(&utility, |app, _| {
        let touch = app.physical_view.touch.unwrap();
        assert_eq!(touch.pull, 1., "pulled as far as it goes");
        assert!(touch.at > 0.3);
    });
    harness::release(&mut utility, pulled);
    harness::paint(&app, &mut utility);
    app.read_with(&utility, |app, _| {
        assert!(app.physical_view.touch.is_none());
        assert_eq!(
            app.physical_view.ringing,
            Some(60),
            "the plucked note rings on"
        );
        // A touch is a performance, not an edit.
        assert_eq!(app.project(), &before);
    });
    // The next touch damps the last one before it plays.
    harness::press(&mut utility, grab);
    app.read_with(&utility, |app, _| {
        assert!(app.physical_view.ringing.is_none())
    });
    harness::release(&mut utility, grab);
    utility.simulate_keystrokes("escape");
    harness::paint(&app, cx);
    app.read_with(cx, |app, _| {
        assert!(app.physical_view.ringing.is_none());
        assert!(app.physical_view.source.is_none());
    });
}

#[gpui::test]
fn a_bow_and_a_breath_last_as_long_as_the_press(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    for id in ["auris.physical.violin", "auris.physical.clarinet"] {
        let (_, mut utility) = open_model(&app, cx, id);
        let lane = utility.debug_bounds("physical-body-1").unwrap();
        let at = point(lane.left() + lane.size.width * 0.2, lane.top() + px(2.));
        harness::press(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            let touch = app.physical_view.touch.expect("the press is held");
            assert!(touch.sounding, "{id} sounds while pressed");
            assert!(touch.pull > 0.8, "pressed near the top, {}", touch.pull);
        });
        harness::release(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            assert!(app.physical_view.touch.is_none());
            assert!(app.physical_view.ringing.is_none(), "{id} stops on release");
        });
    }
}

#[gpui::test]
fn an_oscillator_is_drawn_as_its_waveform_and_played_like_a_key(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    for (id, pictures) in [
        ("auris.synth.fm2", ["synth-operators", "synth-sidebands"]),
        ("auris.synth.chiptune", ["synth-cycle", "synth-harmonics"]),
        ("auris.synth.supersaw", ["synth-stack", "synth-filter"]),
        ("auris.synth.pluck", ["synth-stack", "synth-filter"]),
        ("auris.synth.boom_bass", ["synth-punch", "synth-drive"]),
        ("auris.synth.sub_bass", ["synth-punch", "synth-harmonics"]),
    ] {
        let (track, mut utility) = open_model(&app, cx, id);
        for picture in pictures {
            assert!(
                utility.debug_bounds(picture).is_some(),
                "{id} draws {picture}"
            );
        }
        // The display gain and trails belong to bodies, not waveforms.
        assert!(utility.debug_bounds("physical-gain").is_none(), "{id}");
        app.update(&mut utility, |app, _| {
            // The patch's own cycle waits on the first lane before anything is played.
            let resting = app.oscillator_resting_cycle(track, id);
            assert_eq!(resting.len(), auris_session::MOTION_POINTS);
            assert!(resting.iter().any(|sample| sample.abs() > 0.5), "{id}");
        });
        let before = app.read_with(&utility, |app, _| app.project().clone());
        let lane = utility.debug_bounds("physical-body-0").unwrap();
        let at = point(lane.center().x, lane.top() + px(2.));
        harness::press(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            let touch = app.physical_view.touch.expect("the key is held");
            assert!(touch.sounding, "{id} sounds as it is pressed");
            assert_eq!(touch.pitch, 60);
        });
        harness::release(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            assert!(app.physical_view.touch.is_none());
            assert!(app.physical_view.ringing.is_none(), "{id} stops on release");
            assert_eq!(app.project(), &before);
        });
        // A sounding voice is drawn from its frame, and the patch follows the knobs.
        app.update(&mut utility, |app, cx| {
            let mut frame = MotionFrame {
                geometry: MotionGeometry::Oscillator,
                active: 1,
                ..Default::default()
            };
            frame.voices[0].pitch = 64.;
            frame.voices[0].level = 0.7;
            frame.voices[0].held = true;
            frame.voices[0].excitation = 2.5;
            for (n, point) in frame.voices[0].points.iter_mut().enumerate() {
                *point = (n as f32 * 0.1).sin() * 0.7;
            }
            app.physical_view.accept_frame(frame);
            app.physical_view.frozen = true;
            cx.notify();
        });
        harness::paint(&app, &mut utility);
        assert!(utility.debug_bounds("physical-body-3").is_some(), "{id}");
    }
}

#[gpui::test]
fn an_electronic_kit_is_drawn_as_waveforms_and_struck_by_pad(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    for id in [
        "auris.synth.electro_kit",
        "auris.synth.edm_kit",
        "auris.synth.trap_kit",
    ] {
        let (track, mut utility) = open_model(&app, cx, id);
        for picture in ["synth-kick", "synth-metal"] {
            assert!(
                utility.debug_bounds(picture).is_some(),
                "{id} draws {picture}"
            );
        }
        // A kit is auditioned by pad, and starts on its kick.
        app.update(&mut utility, |app, _| {
            assert_eq!(app.physical_view.pitch, 36);
            assert!(app.physical_view.drum_preview);
            let resting = app.oscillator_resting_cycle(track, id);
            assert_eq!(resting.len(), auris_session::MOTION_POINTS);
            assert!(resting.iter().any(|sample| sample.abs() > 0.5), "{id}");
        });
        let lane = utility.debug_bounds("physical-body-0").unwrap();
        let at = point(lane.center().x, lane.top() + px(2.));
        harness::press(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            let touch = app.physical_view.touch.expect("the pad is struck");
            assert!(touch.sounding, "{id} sounds as it is struck");
            assert_eq!(touch.pitch, 36);
        });
        harness::release(&mut utility, at);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            // A struck pad rings on after the press, as a drum does.
            assert_eq!(app.physical_view.ringing, Some(36), "{id}");
        });
    }
}

#[gpui::test]
fn each_body_is_drawn_as_the_thing_it_is(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    for (id, geometry, cells) in [
        ("auris.physical.piano", MotionGeometry::String, false),
        ("auris.physical.mallet", MotionGeometry::Bar, false),
        ("auris.physical.bell", MotionGeometry::Shell, true),
        ("auris.physical.clarinet", MotionGeometry::Bore, false),
        ("auris.physical.tin_whistle", MotionGeometry::Bore, false),
        ("auris.physical.choir", MotionGeometry::Tract, false),
    ] {
        let (_, mut utility) = open_model(&app, cx, id);
        // At rest, a bell is a row of rims and everything else a lane under another.
        let first = utility.debug_bounds("physical-body-0").unwrap();
        let second = utility.debug_bounds("physical-body-1").unwrap();
        assert_eq!(
            (second.top() - first.top()).abs() < px(1.),
            cells,
            "{id} lays its bodies out side by side"
        );
        app.update(&mut utility, |app, cx| {
            let mut frame = MotionFrame {
                geometry,
                active: 2,
                pedal: true,
                ..Default::default()
            };
            for (index, voice) in frame.voices.iter_mut().take(2).enumerate() {
                voice.pitch = 60. + index as f32 * 7.;
                voice.level = 0.6;
                voice.held = index == 0;
                voice.contact = 0.2;
                voice.excitation = 0.5;
                for (n, point) in voice.points.iter_mut().enumerate() {
                    *point = (n as f32 * 0.3).sin() * 0.04;
                }
                voice.modes = [0.9, 0.7, 0.5, 0.4, 0.6, 0.8, 0.6, 0.3];
            }
            app.physical_view.accept_frame(frame);
            app.physical_view.frozen = true;
            cx.notify();
        });
        harness::paint(&app, &mut utility);
        assert!(utility.debug_bounds("physical-body-3").is_some(), "{id}");
    }
}

#[gpui::test]
fn a_bell_is_struck_around_its_rim(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    let (_, mut utility) = open_model(&app, cx, "auris.physical.bell");
    let cell = utility.debug_bounds("physical-body-0").unwrap();
    let (radius, _) = super::super::physical_bodies::ring_radius(cell);
    let right = point(cell.center().x + radius, cell.center().y);
    harness::press(&mut utility, right);
    app.read_with(&utility, |app, _| {
        let touch = app.physical_view.touch.expect("the rim is struck");
        assert!(touch.sounding, "a bell sounds as it is struck");
        assert!(
            (touch.at - 0.25).abs() < 0.01,
            "a quarter turn, {}",
            touch.at
        );
    });
    harness::release(&mut utility, right);
    app.read_with(&utility, |app, _| {
        assert_eq!(app.physical_view.ringing, Some(60), "and rings on");
    });
}

#[gpui::test]
fn a_drum_head_is_struck_where_it_is_pressed(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    let track = app.update(cx, |app, cx| {
        let track = app.session.add_default_drum_track("Kit").unwrap();
        app.open_plugin_window(PluginSubject::Instrument(track));
        cx.notify();
        track
    });
    harness::paint(&app, cx);
    let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
    let mut utility = VisualTestContext::from_window(handle.into(), cx);
    // Tall enough that the audition card is on screen below the kit and its modes.
    harness::resize(&app, &mut utility, size(px(1080.), px(1100.)));
    app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 36));
    let head = utility.debug_bounds("physical-body-0").unwrap();
    harness::press(&mut utility, head.center());
    app.read_with(&utility, |app, _| {
        let touch = app.physical_view.touch.unwrap();
        assert!(touch.sounding && touch.at < 0.05, "struck at the centre");
        assert_eq!(touch.pitch, 36);
    });
    harness::release(&mut utility, head.center());
    // Outside the circle is not the head.
    harness::press(&mut utility, head.origin + point(px(1.), px(1.)));
    app.read_with(&utility, |app, _| {
        assert!(app.physical_view.touch.is_none());
        assert_eq!(app.physical_view.ringing, Some(36));
    });
    harness::release(&mut utility, head.origin + point(px(1.), px(1.)));
    harness::click("physical-octave-up", &mut utility);
    app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 38));
    harness::click("physical-play", &mut utility);
    app.read_with(&utility, |app, _| assert!(!app.physical_view.playing));
    utility.simulate_keystrokes("escape");
    harness::paint(&app, cx);
    app.read_with(cx, |app, _| {
        assert!(app.plugin_window.is_none());
        assert!(app.session.has_instrument_motion(track));
    });
}

#[gpui::test]
fn physical_editor_supports_preview_freeze_parameter_undo_and_escape(
    cx: &mut gpui::TestAppContext,
) {
    let (app, cx) = harness::open(cx);
    let (track, target, descriptor, before) = app.update(cx, |app, cx| {
        let track = app
            .session
            .add_instrument_track("Violin", "auris.physical.violin")
            .unwrap();
        let descriptor = app
            .session
            .instrument_descriptors(track)
            .iter()
            .find(|d| d.key == "bow_response")
            .unwrap()
            .clone();
        let target = ParamTarget::Instrument {
            track,
            param: descriptor.id,
        };
        let before = app.session.param_value(target, &descriptor);
        app.open_plugin_window(PluginSubject::Instrument(track));
        cx.notify();
        (track, target, descriptor, before)
    });
    harness::paint(&app, cx);
    let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
    let mut utility = VisualTestContext::from_window(handle.into(), cx);
    assert!(utility.debug_bounds("physical-motion").is_some());
    harness::click("physical-gain", &mut utility);
    app.read_with(&utility, |app, _| assert_eq!(app.physical_view.gain, 96.));
    // Re-enter the native button through the real tab order. Mouse activation may leave
    // focus on the button, so walk out and back in before testing keyboard activation.
    utility.simulate_keystrokes("shift-tab tab");
    utility.simulate_keystrokes("enter");
    utility.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    app.read_with(&utility, |app, _| assert_eq!(app.physical_view.gain, 6.));
    harness::resize(&app, &mut utility, size(px(1080.), px(900.)));
    harness::click("physical-play", &mut utility);
    app.read_with(&utility, |app, _| assert!(app.physical_view.playing));
    harness::click("physical-octave-up", &mut utility);
    app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 72));
    harness::resize(&app, &mut utility, size(px(520.), px(480.)));
    // No audio device exists in this harness: supply a captured-frame fixture for the
    // presentation-only resume gesture. DSP-to-monitor integration has numerical tests.
    app.update(&mut utility, |app, cx| {
        app.physical_view.frame = Some(MotionFrame {
            active: 1,
            ..Default::default()
        });
        app.toggle_physical_freeze();
        cx.notify();
    });
    app.read_with(&utility, |app, _| assert!(app.physical_view.frozen));
    harness::click("physical-freeze", &mut utility);
    app.read_with(&utility, |app, _| assert!(!app.physical_view.frozen));
    let body = utility.debug_bounds("pw-body").unwrap();
    utility.simulate_event(gpui::ScrollWheelEvent {
        position: body.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-10000.))),
        ..Default::default()
    });
    harness::paint(&app, &mut utility);
    let selector = Box::leak(format!("pw-inst-param-{}", descriptor.id.0).into_boxed_str());
    let bounds = utility.debug_bounds(selector).unwrap();
    harness::drag(
        &mut utility,
        bounds.center(),
        bounds.center() + point(px(60.), px(0.)),
    );
    app.read_with(&utility, |app, _| {
        assert_ne!(app.session.param_value(target, &descriptor), before)
    });
    utility.dispatch_action(crate::actions::Undo);
    app.read_with(&utility, |app, _| {
        assert_eq!(app.session.param_value(target, &descriptor), before)
    });
    utility.simulate_keystrokes("escape");
    harness::paint(&app, cx);
    app.read_with(cx, |app, _| {
        assert!(!app.physical_view.playing);
        assert!(app.plugin_window.is_none());
        assert!(app.session.has_instrument_motion(track));
    });
}

#[gpui::test]
fn nonphysical_instrument_replaces_the_motion_surface_and_releases_preview(
    cx: &mut gpui::TestAppContext,
) {
    let (app, cx) = harness::open(cx);
    app.update(cx, |app, cx| {
        let physical = app
            .session
            .add_instrument_track("Guitar", "auris.physical.guitar")
            .unwrap();
        app.open_plugin_window(PluginSubject::Instrument(physical));
        app.poll_physical_view();
        app.toggle_physical_preview(cx);
        assert!(app.physical_view.playing);
        app.physical_view.ringing = Some(64);
        app.physical_view.effects_disabled = true;
        app.physical_view.accept_frame(MotionFrame {
            active: 1,
            ..Default::default()
        });
        assert!(!app.physical_view.history.is_empty());
        let plain = app
            .session
            .add_instrument_track("Noise", "auris.synth.noisedrum")
            .unwrap();
        app.open_plugin_window(PluginSubject::Instrument(plain));
        app.poll_physical_view();
        assert!(!app.physical_view.playing);
        assert!(app.physical_view.ringing.is_none());
        assert!(app.physical_view.source.is_none());
        assert!(app.physical_view.frame.is_none());
        assert!(app.physical_view.history.is_empty());
        assert!(app.physical_view.effects_disabled);
        cx.notify();
    });
    harness::paint(&app, cx);
}

/// Reads one of a track's instrument parameters by key.
fn param(app: &Entity<AurisApp>, cx: &mut VisualTestContext, track: TrackId, key: &str) -> f32 {
    app.update(cx, |app, _| app.instrument_param(track, key).unwrap())
}

/// Opens `id` in a window tall enough that the patch's pictures are on screen without
/// scrolling.
fn open_tall(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    id: &str,
) -> (TrackId, VisualTestContext) {
    let (track, mut utility) = open_model(app, cx, id);
    harness::resize(app, &mut utility, size(px(1080.), px(1600.)));
    harness::paint(app, &mut utility);
    (track, utility)
}

/// Drags across a picture from its centre by `(dx, dy)` pixels and lets go.
fn drag_plot(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    selector: &'static str,
    from: (f32, f32),
    by: (f32, f32),
) {
    let bounds = cx.debug_bounds(selector).expect(selector);
    assert!(
        app.read_with(cx, |app, _| app.drag.is_none()),
        "nothing is held before {selector}"
    );
    let at = point(
        bounds.center().x + px(from.0),
        bounds.center().y + px(from.1),
    );
    let to = point(at.x + px(by.0), at.y + px(by.1));
    harness::press(cx, at);
    harness::paint(app, cx);
    harness::drag_to(cx, point(at.x + px(by.0 / 2.), at.y + px(by.1 / 2.)));
    harness::paint(app, cx);
    harness::drag_to(cx, to);
    harness::paint(app, cx);
    harness::release(cx, to);
    harness::paint(app, cx);
}

#[gpui::test]
fn the_subtractive_pictures_are_dragged_like_curves(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    let (track, mut utility) = open_tall(&app, cx, "auris.synth.pad");
    let cutoff = param(&app, &mut utility, track, "cutoff");
    let resonance = param(&app, &mut utility, track, "resonance");
    // Right and up: the knee moves to a higher harmonic and peaks harder.
    drag_plot(&app, &mut utility, "synth-filter", (0., 0.), (60., -30.));
    let opened = param(&app, &mut utility, track, "cutoff");
    let peaked = param(&app, &mut utility, track, "resonance");
    assert!(opened > cutoff + 100., "{cutoff} Hz → {opened} Hz");
    assert!(peaked > resonance + 0.1, "{resonance} → {peaked}");
    // The whole drag, both parameters, is one step back.
    app.update(&mut utility, |app, _| {
        app.session.undo();
    });
    assert_eq!(param(&app, &mut utility, track, "cutoff"), cutoff);
    assert_eq!(param(&app, &mut utility, track, "resonance"), resonance);

    // A stack widens whichever side it is pulled out from, and the axis holds still under it.
    let detune = param(&app, &mut utility, track, "detune");
    drag_plot(&app, &mut utility, "synth-stack", (-40., 0.), (-30., 0.));
    let left = param(&app, &mut utility, track, "detune");
    assert!(left > detune, "pulled left: {detune} → {left}");
    drag_plot(&app, &mut utility, "synth-stack", (40., 0.), (30., 0.));
    let right = param(&app, &mut utility, track, "detune");
    assert!(right > left, "pulled right: {left} → {right}");
    // Down lowers the side copies.
    let mix = param(&app, &mut utility, track, "mix");
    drag_plot(&app, &mut utility, "synth-stack", (40., 0.), (0., 20.));
    assert!(param(&app, &mut utility, track, "mix") < mix);

    // The wheel adds a copy a notch.
    let copies = param(&app, &mut utility, track, "unison");
    let stack = utility.debug_bounds("synth-stack").unwrap();
    utility.simulate_event(gpui::ScrollWheelEvent {
        position: stack.center(),
        delta: gpui::ScrollDelta::Lines(point(0., -1.)),
        ..Default::default()
    });
    harness::paint(&app, &mut utility);
    let after = param(&app, &mut utility, track, "unison");
    assert_eq!((after - copies).abs(), 1.0, "{copies} → {after}");

    // A double click puts the filter back where the patch has it.
    drag_plot(&app, &mut utility, "synth-filter", (0., 0.), (40., -20.));
    let filter = utility.debug_bounds("synth-filter").unwrap();
    harness::double_click(&mut utility, filter.center());
    harness::paint(&app, &mut utility);
    let defaults = app.update(&mut utility, |app, _| {
        let descriptors = app.session.instrument_descriptors(track);
        let default = |key: &str| {
            descriptors
                .iter()
                .find(|descriptor| descriptor.key == key)
                .unwrap()
                .default
        };
        (default("cutoff"), default("resonance"))
    });
    assert_eq!(param(&app, &mut utility, track, "cutoff"), defaults.0);
    assert_eq!(param(&app, &mut utility, track, "resonance"), defaults.1);
}

#[gpui::test]
fn the_fm_and_chiptune_pictures_take_a_drag(cx: &mut gpui::TestAppContext) {
    let (app, cx) = harness::open(cx);
    let (track, mut utility) = open_tall(&app, cx, "auris.synth.fm2");
    let ratio = param(&app, &mut utility, track, "ratio");
    let index = param(&app, &mut utility, track, "index");
    drag_plot(&app, &mut utility, "synth-sidebands", (0., 0.), (40., -20.));
    assert!(param(&app, &mut utility, track, "ratio") > ratio);
    assert!(param(&app, &mut utility, track, "index") > index);

    let (track, mut utility) = open_tall(&app, cx, "auris.synth.chiptune");
    assert!(utility.debug_bounds("synth-chip-stack").is_some());
    let width = param(&app, &mut utility, track, "pulse_width");
    let bits = param(&app, &mut utility, track, "bits");
    // The default square: sideways moves the duty cycle, down crushes the bits.
    drag_plot(&app, &mut utility, "synth-cycle", (0., 0.), (-40., 40.));
    assert!(param(&app, &mut utility, track, "pulse_width") < width);
    assert!(param(&app, &mut utility, track, "bits") < bits);
    let spread = param(&app, &mut utility, track, "spread");
    drag_plot(&app, &mut utility, "synth-chip-stack", (30., 0.), (40., 0.));
    assert!(param(&app, &mut utility, track, "spread") > spread);
}
