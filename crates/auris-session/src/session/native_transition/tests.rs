use super::*;
use crate::native_edit::{NativeEdit, NativeEditOwner};
use crate::{Edit, SessionOptions};
use auris_core::pipeline::content::{PluginCapabilities, Processor};

struct NoPlugins;
impl CapabilityProvider for NoPlugins {
    fn plugin<'a>(&'a self, _: &Processor) -> Option<PluginCapabilities<'a>> {
        None
    }
}

fn session() -> Session {
    Session::new(SessionOptions {
        shipped_fonts: false,
        shipped_dictionary: false,
        ..SessionOptions::headless()
    })
    .unwrap()
}

fn graph_limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 512,
        edges: 1024,
        ports: 4096,
        parameter_keys: 256,
        text_bytes: 1 << 20,
    }
}

#[test]
fn native_preparation_reuses_session_runtime_without_publishing_readers_or_history() {
    let mut session = session();
    let track = session.add_bus_track("Bus");
    session.forget_history();
    let source = session.project().clone();
    let meters = session.signal_meters.clone();
    let scope = session.scope.clone();
    let revision = session.revision();
    let mut transition = session
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let nodes = transition.editor().document().content.processors.clone();
    transition
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Native bus".into(),
            }],
        )
        .unwrap();
    let generation = session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    assert_eq!(transition.prepared_generation(), Some(generation));
    assert_eq!(transition.editor().document().content.processors, nodes);
    assert_eq!(session.project(), &source);
    assert_eq!(session.revision(), revision);
    assert!(!session.is_dirty());
    assert!(!session.can_undo());
    assert!(std::sync::Arc::ptr_eq(&session.signal_meters, &meters));
    assert!(std::sync::Arc::ptr_eq(&session.scope, &scope));
    assert_eq!(session.engine.adopted_native_generation(), None);
    assert!(!session.engine.is_running());
}

#[test]
fn conventional_edits_or_another_session_reject_the_captured_transition() {
    let mut session = session();
    let mut transition = session
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let mut other = self::session();
    assert!(matches!(
        other.prepare_native_transition(&mut transition, &NoPlugins, graph_limits()),
        Err(NativeTransitionError::Stale)
    ));
    session.add_bus_track("Later");
    assert!(matches!(
        session.prepare_native_transition(&mut transition, &NoPlugins, graph_limits()),
        Err(NativeTransitionError::Stale)
    ));
    assert!(transition.prepared_generation().is_none());
}

#[test]
fn digest_catches_source_changes_that_did_not_advance_the_edit_counter() {
    let mut session = session();
    let mut transition = session
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let revision = session.revision();
    session.project.name = "Changed without record".into();
    assert_eq!(session.revision(), revision);
    assert!(matches!(
        session.prepare_native_transition(&mut transition, &NoPlugins, graph_limits()),
        Err(NativeTransitionError::Stale)
    ));
}

#[test]
fn a_gesture_blocks_capture_without_finishing_it() {
    let mut session = session();
    session.begin_transaction(Edit::ExternalChanges);
    assert!(matches!(
        session.capture_native_transition(crate::native_edit::tests::limits()),
        Err(NativeTransitionError::ActiveEdit)
    ));
    assert!(session.transaction.is_some());
    session.revert_transaction();
    assert!(
        session
            .capture_native_transition(crate::native_edit::tests::limits())
            .is_ok()
    );
}

#[test]
fn editing_the_candidate_invalidates_its_prepared_revision_and_retains_ids() {
    let mut session = session();
    let track = session.add_bus_track("Bus");
    let mut transition = session
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let generation = session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    let processors = transition.editor().document().content.processors.clone();
    transition
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Agent(8),
            Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Candidate edit".into(),
            }],
        )
        .unwrap();
    assert_eq!(transition.prepared_generation(), None);
    let next = session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    assert_ne!(next, generation);
    assert_eq!(
        transition.editor().document().content.processors,
        processors
    );
}

#[test]
fn native_commands_reach_the_session_prepared_audio_and_undo_restores_samples() {
    use auris_core::{AudioBuffer, Note, Ticks};
    use auris_engine::{Transport, render_block};

    fn render(transition: &mut NativeSessionTransition) -> AudioBuffer {
        let graph = transition
            .hosts
            .as_mut()
            .unwrap()
            .prepared_graph_mut()
            .unwrap();
        let mut buffer = AudioBuffer::new(2, 4096, 48_000.0);
        render_block(graph, &mut Transport::playing_from(0), &mut buffer, true);
        buffer
    }

    fn rms(buffer: &AudioBuffer) -> f64 {
        (buffer
            .channel(0)
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / buffer.channel(0).len() as f64)
            .sqrt()
    }

    let mut session = session();
    let track = session
        .add_instrument_track("Chiptune", auris_synth::Chiptune::ID)
        .unwrap();
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    session
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    let source = session.project().clone();
    let mut transition = session
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let provider =
        crate::native_conventional::tests::builtin_provider(transition.editor().document());
    session
        .prepare_native_transition(&mut transition, &provider, graph_limits())
        .unwrap();
    let original = render(&mut transition);
    let baseline = rms(&original);
    assert!(baseline.is_finite() && baseline > 1e-6);
    let document = transition.editor().document();
    let strip = document
        .content
        .groups
        .iter()
        .find(|group| group.id == document.content.master)
        .unwrap()
        .strip;
    transition
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::SetStrip {
                node: strip,
                gain_db: Some(-6.0),
                pan: None,
                mute: None,
                solo: None,
            }],
        )
        .unwrap();
    session
        .prepare_native_transition(&mut transition, &provider, graph_limits())
        .unwrap();
    let quieter = rms(&render(&mut transition));
    assert!(
        (quieter / baseline - 10.0_f64.powf(-6.0 / 20.0)).abs() < 1e-5,
        "baseline={baseline}, quieter={quieter}"
    );
    transition.editor_mut().undo(1).unwrap();
    session
        .prepare_native_transition(&mut transition, &provider, graph_limits())
        .unwrap();
    let restored = render(&mut transition);
    assert_eq!(restored.channel(0), original.channel(0));
    assert_eq!(restored.channel(1), original.channel(1));
    assert_eq!(session.project(), &source);
    assert_eq!(session.engine.adopted_native_generation(), None);
}
