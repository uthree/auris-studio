//! The choir is an ordinary instrument through discovery, editing, saving and rendering.

use super::fixtures::{Scratch, session};
use crate::prelude::*;

#[test]
fn choir_parameters_and_automation_round_trip_and_render_without_assets() {
    let scratch = Scratch::new("physical-choir");
    let mut live = session();
    let sounds: serde_json::Value = serde_json::from_str(
        &live
            .agent_instrument_list(Some("choir"), 0, false, &[])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(sounds["sounds"][0]["id"], "auris.physical.choir");
    let track = live
        .add_instrument_track("Choir", "auris.physical.choir")
        .unwrap();
    let clip = live
        .add_midi_clip(track, "Chord", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    for pitch in [48, 55, 60] {
        live.add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::from_beats(2.0)))
            .unwrap();
    }
    let vowel = live
        .instrument_descriptors(track)
        .iter()
        .find(|p| p.key == "vowel")
        .unwrap()
        .clone();
    let target = ParamTarget::Instrument {
        track,
        param: vowel.id,
    };
    live.set_param(target, 0.5);
    live.set_automation_point(target, Ticks::ZERO, 0.0);
    live.set_automation_point(target, Ticks::from_beats(2.0), 2.0);
    let path = live.save_as(&scratch.join("Choir.auris")).unwrap().document;
    let options = OfflineOptions {
        include_tail: false,
        ..Default::default()
    };
    let before = live
        .render_job()
        .render(&options, &mut Default::default())
        .unwrap();

    let mut reopened = session();
    assert!(reopened.open(&path).unwrap().is_empty());
    assert!(reopened.project().audio_sources.is_empty());
    assert!(reopened.project().soundfonts.is_empty());
    assert!(
        reopened
            .project()
            .track(track)
            .unwrap()
            .kind
            .is_instrument()
    );
    assert_eq!(reopened.param_value(target, &vowel), 0.5);
    assert_eq!(live.project().automation, reopened.project().automation);
    let after = reopened
        .render_job()
        .render(&options, &mut Default::default())
        .unwrap();
    assert!(after.peak() > 0.01);
    assert_eq!(before.channels(), after.channels());
}
