//! Opt-in check that an installed VST3 instrument survives a render instance per edit.

use std::path::PathBuf;

use auris_core::{Note, Ticks};
use auris_engine::OfflineOptions;
use auris_session::{Session, SessionOptions};

/// Every edit rebuilds the graph, and every rebuild builds a render instance of each hosted
/// plugin and retires the one before it. Ample Guitar M Lite crashed the process when an
/// instance was released within milliseconds of its last call, which this did within a few
/// hundred edits on most runs; set `AURIS_VST3_SMOKE_PLUGIN` to its bundle to check it.
#[test]
#[ignore = "requires AURIS_VST3_SMOKE_PLUGIN to name an installed VST3 instrument"]
fn an_installed_instrument_survives_a_render_instance_per_edit() {
    let path = PathBuf::from(
        std::env::var_os("AURIS_VST3_SMOKE_PLUGIN")
            .expect("AURIS_VST3_SMOKE_PLUGIN must name a VST3 bundle"),
    );
    let mut session =
        Session::new(SessionOptions::headless().with_plugin_paths(vec![path.clone()])).unwrap();
    let info = session
        .vst3_plugins_in(&path)
        .unwrap()
        .into_iter()
        .next()
        .expect("the bundle must expose an audio class");
    let track = session.add_default_instrument_track("Rebuilds").unwrap();
    session
        .set_vst3_instrument(track, &path, &info.class_id)
        .unwrap();

    const BEATS: usize = 48;
    let clip = session
        .add_midi_clip(
            track,
            "Chords",
            Ticks::ZERO,
            Ticks::from_beats(BEATS as f64),
        )
        .unwrap();
    for beat in 0..BEATS {
        for pitch in [40, 47, 52, 55, 59, 64] {
            let start = Ticks::from_beats(beat as f64);
            let note = Note::new(pitch + (beat % 3) as u8, start, Ticks::from_beats(0.9));
            session.add_note(clip, note).unwrap();
        }
    }

    let audio = session
        .render_job()
        .render(&OfflineOptions::whole_project(), &mut Default::default())
        .unwrap();
    assert!(
        audio.channel(0).iter().all(|sample| sample.is_finite()),
        "the render should be finite"
    );
}
