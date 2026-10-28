use std::sync::Arc;

use auris_core::ParamTarget;
use auris_core::Ticks;
use auris_core::automation::AutomationCurve;
use auris_core::pipeline::content::ContentLimits;
use auris_core::project::{Project, TrackKind};

use super::{
    NativePersistenceError, native_to_project, project_to_native_preserving,
    validate_lossless_conventional,
};

fn limits() -> ContentLimits {
    crate::project_file::SESSION_CONTENT_LIMITS
}

fn rich_project() -> Project {
    let mut project = Project::new("codec", 48_000.0);
    project.metronome = true;
    project.loop_enabled = true;
    let instrument = project.add_instrument_track("lead", "auris.test.instrument");
    let singer = project.add_singer_track("voice", "auris.test.preview");
    let audio = project.add_audio_track("recording");
    let bus = project.add_bus_track("bus");
    let source = project.add_audio_source(
        "take",
        auris_core::AssetPath::inside("Audio/take.wav"),
        48_000,
        48_000.0,
        2,
    );
    project.add_audio_clip(audio, source, Ticks::ZERO).unwrap();
    project.add_effect(Some(instrument), "auris.test.effect");
    project.add_effect(None, "auris.test.master");
    let send = auris_core::SendId(project.next_id());
    assert!(project.restore_allocation_counter(send.0 + 1));
    let lead = project.track_mut(instrument).unwrap();
    lead.outputs.push(auris_core::Output::Bus(bus));
    lead.mixer.effects[0].sidechain = Some(audio);
    lead.sends.push(auris_core::AuxSend {
        id: send,
        target: bus,
        level_db: -9.0,
        pre_fader: true,
    });
    project.master.effects[0].sidechain = Some(audio);
    project
        .track_mut(instrument)
        .expect("instrument")
        .kind
        .as_instrument_mut()
        .expect("instrument kind")
        .instrument_state
        .params
        .insert("cutoff".into(), 0.5);
    project
        .add_midi_clip(instrument, "lead", Ticks::ZERO, Ticks::from_beats(4.0))
        .expect("lead clip");
    project
        .add_midi_clip(singer, "voice", Ticks::ZERO, Ticks::from_beats(2.0))
        .expect("singer clip");
    project.automation.set_point(
        ParamTarget::TrackGain(instrument),
        None,
        AutomationCurve::Linear,
        Ticks::ZERO,
        -3.0,
    );
    project.automation.set_point(
        ParamTarget::Instrument {
            track: instrument,
            param: 0.into(),
        },
        Some("cutoff".into()),
        AutomationCurve::Linear,
        Ticks::ZERO,
        0.25,
    );
    assert!(matches!(
        project.track(bus).map(|track| &track.kind),
        Some(TrackKind::Bus)
    ));
    project
}

#[test]
fn native_codec_round_trips_context_tracks_and_automation() {
    let project = rich_project();
    let native = project_to_native_preserving(&project, limits()).expect("encode");
    let restored = native_to_project(&native, limits()).expect("lossless decode");
    assert_eq!(restored.name, project.name);
    assert_eq!(restored.sample_rate, project.sample_rate);
    assert_eq!(restored.tempo_map, project.tempo_map);
    assert_eq!(restored.signatures, project.signatures);
    assert_eq!(restored.harmony, project.harmony);
    assert_eq!(restored.sections, project.sections);
    assert_eq!(restored.tracks.len(), project.tracks.len());
    assert_eq!(restored.tracks, project.tracks);
    assert_eq!(restored.master, project.master);
    assert_eq!(
        restored.automation.lanes().len(),
        project.automation.lanes().len()
    );
}

#[test]
fn native_codec_rejects_shared_source() {
    let project = rich_project();
    let mut native = project_to_native_preserving(&project, limits()).expect("encode");
    let source = native.content.tracks[0].source.expect("source");
    native.content.tracks[1].source = Some(source);
    assert!(matches!(
        validate_lossless_conventional(&native, limits()),
        Err(NativePersistenceError::SharedSource(_))
    ));
}

#[test]
fn native_codec_rejects_disabled_and_unbound_automation() {
    let project = rich_project();
    let mut native = project_to_native_preserving(&project, limits()).expect("encode");
    native.content.automation_bindings[0].enabled = false;
    assert!(matches!(
        validate_lossless_conventional(&native, limits()),
        Err(NativePersistenceError::DisabledAutomation(_))
    ));

    let mut native = project_to_native_preserving(&project, limits()).expect("encode");
    native.content.automation_bindings.clear();
    assert!(matches!(
        validate_lossless_conventional(&native, limits()),
        Err(NativePersistenceError::ProjectionUnsupported(
            "unbound automation curve"
        ))
    ));
}

#[test]
fn native_codec_rejects_insert_cycle_without_hanging() {
    let project = rich_project();
    let mut native = project_to_native_preserving(&project, limits()).expect("encode");
    let strip = native
        .content
        .groups
        .iter()
        .find(|group| group.id != native.content.master)
        .expect("track group")
        .strip;
    let input = native
        .content
        .processors
        .iter()
        .find(|processor| {
            processor.group
                == Some(
                    native
                        .content
                        .groups
                        .iter()
                        .find(|group| group.strip == strip)
                        .expect("group")
                        .id,
                )
                && matches!(
                    &processor.kind,
                    auris_core::pipeline::content::ProcessorKind::Sum
                )
        })
        .expect("sum")
        .id;
    let edge = native
        .content
        .edges
        .iter_mut()
        .find(|edge| edge.to.0 == strip)
        .expect("input edge");
    edge.to.0 = input;
    assert!(native_to_project(&native, limits()).is_err());
}

#[test]
fn native_identity_checkpoint_is_available_for_subsequent_edits() {
    let project = rich_project();
    let native = project_to_native_preserving(&project, limits()).expect("encode");
    let mut project = project;
    project.set_native_identity(Arc::new(native.clone()));
    let new_track = project.add_audio_track("later");
    assert!(project.track(new_track).is_some());
    assert!(project.native_identity().is_some());
    assert!(project.native_allocation_start().expect("allocation start") >= native.content.next_id);
    let edited = project_to_native_preserving(&project, limits()).unwrap();
    let reopened = native_to_project(&edited, limits()).unwrap();
    assert_eq!(project, reopened);
    for node in &native.content.processors {
        assert!(
            edited
                .content
                .processors
                .iter()
                .any(|current| current.id == node.id)
        );
    }
}
