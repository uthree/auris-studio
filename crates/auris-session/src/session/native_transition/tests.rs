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
fn native_routing_and_automation_render_through_session_and_reopen_with_stable_ids() {
    use crate::native_edit::{NativeCreated, NativeRoutingEdit};
    use crate::native_file_session::NativeRecordingFile;
    use auris_core::{AssetPath, AudioBuffer, ParamTarget, Ticks};
    use auris_engine::{Transport, render_block};

    fn samples(transition: &mut NativeSessionTransition) -> AudioBuffer {
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
    fn power(buffer: &AudioBuffer) -> f64 {
        buffer
            .channel(0)
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
    }
    let directory = tempfile::tempdir().unwrap();
    let mut session = session();
    let track = session.add_audio_track("Take");
    let main_bus = session.add_bus_track("Main branch");
    let send_bus = session.add_bus_track("Aux branch");
    let folder = session.project_folder().unwrap().to_path_buf();
    std::fs::create_dir_all(folder.join("Audio")).unwrap();
    let mut writer =
        auris_io::WavRecorder::create(&folder.join("Audio/take.wav"), 48_000.0, 1).unwrap();
    writer
        .write(
            &(0..8192)
                .map(|n| (n as f32 * 0.05).sin() * 0.02)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    writer.finish().unwrap();
    let limits = crate::native_edit::tests::limits();
    let mut transition = session.capture_native_transition(limits).unwrap();
    transition
        .files_mut()
        .land_recorded_files(
            0,
            NativeEditOwner::Human,
            &[NativeRecordingFile {
                track,
                path: AssetPath::inside("Audio/take.wav"),
                start: Ticks::ZERO,
                offset_frames: 0,
                length_frames: None,
            }],
        )
        .unwrap();
    let group = |track| {
        transition
            .editor()
            .document()
            .content
            .tracks
            .iter()
            .find(|binding| binding.track == track)
            .unwrap()
            .group
    };
    let main = group(main_bus);
    let aux = group(send_bus);
    let receipt = transition
        .editor_mut()
        .apply(
            1,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[
                NativeEdit::Routing(NativeRoutingEdit::SetMainRoute {
                    track,
                    destination: main,
                }),
                NativeEdit::Routing(NativeRoutingEdit::AddSend {
                    track,
                    destination: aux,
                    level_db: -6.0,
                    pre_fader: false,
                }),
            ],
        )
        .unwrap();
    let send = receipt
        .created
        .iter()
        .find_map(|id| {
            if let NativeCreated::Send(send) = id {
                Some(*send)
            } else {
                None
            }
        })
        .unwrap();
    session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    let baseline = samples(&mut transition);
    assert!(power(&baseline) > 0.01);
    transition
        .editor_mut()
        .set_automation_point(
            2,
            NativeEditOwner::Human,
            ParamTarget::TrackGain(track),
            Ticks::ZERO,
            -12.0,
        )
        .unwrap();
    session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    let post = samples(&mut transition);
    let ratio = (power(&post) / power(&baseline)).sqrt();
    assert!(
        (ratio - 10.0_f64.powf(-12.0 / 20.0)).abs() < 0.01,
        "post-fader gain ratio {ratio}"
    );
    transition
        .editor_mut()
        .apply(
            3,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::SetSendPreFader {
                send,
                pre_fader: true,
            })],
        )
        .unwrap();
    session
        .prepare_native_transition(&mut transition, &NoPlugins, graph_limits())
        .unwrap();
    let pre = samples(&mut transition);
    assert!(power(&pre) > power(&post) * 2.0);
    assert!(power(&pre) < power(&baseline));
    let document = transition.editor().document().clone();
    let save = transition
        .files_mut()
        .begin_save_as(&directory.path().join("Routed.auris"))
        .unwrap()
        .run()
        .unwrap();
    transition.files_mut().finish_save(save).unwrap();
    let mut reopened = session
        .open_native_transition(transition.files().path().unwrap(), limits)
        .unwrap();
    assert_eq!(reopened.editor().document(), &document);
    session
        .prepare_native_transition(&mut reopened, &NoPlugins, graph_limits())
        .unwrap();
    let output = samples(&mut reopened);
    assert_eq!(output.channel(0), pre.channel(0));
    assert_eq!(output.channel(1), pre.channel(1));
    assert_eq!(session.engine.adopted_native_generation(), None);
}

#[test]
fn recorded_native_candidate_edit_recovery_save_as_reopen_preserves_render_and_ids() {
    use crate::native_edit::{NativeClipEdit, NativeCreated};
    use crate::native_file_session::NativeRecordingFile;
    use auris_core::{AssetPath, AudioBuffer, Ticks};
    use auris_engine::{Transport, render_block};

    fn samples(transition: &mut NativeSessionTransition) -> AudioBuffer {
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
    let directory = tempfile::tempdir().unwrap();
    let mut session = session();
    let track = session.add_audio_track("Recorded");
    session
        .save_as(&directory.path().join("Legacy.auris"))
        .unwrap();
    let original_path = session.path().unwrap().to_path_buf();
    let original_bytes = std::fs::read(&original_path).unwrap();
    let original = session.project().clone();
    let folder = session.project_folder().unwrap().to_path_buf();
    std::fs::create_dir_all(folder.join("Audio")).unwrap();
    let wav = folder.join("Audio/take.wav");
    let mut recorder = auris_io::WavRecorder::create(&wav, 48_000.0, 1).unwrap();
    let mut input = vec![0.0; 2048];
    input.extend((0..8192).map(|n| (n as f32 * 0.05).sin() * 0.25));
    recorder.write(&input).unwrap();
    assert_eq!(recorder.finish().unwrap(), input.len() as u64);

    let limits = crate::native_edit::tests::limits();
    let mut candidate = session.capture_native_transition(limits).unwrap();
    let receipt = candidate
        .files_mut()
        .land_recorded_files(
            0,
            NativeEditOwner::Human,
            &[NativeRecordingFile {
                track,
                path: AssetPath::inside("Audio/take.wav"),
                start: Ticks::ZERO,
                offset_frames: 2048,
                length_frames: Some(4096),
            }],
        )
        .unwrap();
    let clip = receipt
        .created
        .iter()
        .find_map(|id| match id {
            NativeCreated::Clip(id) => Some(*id),
            _ => None,
        })
        .unwrap();
    let source = candidate
        .editor()
        .document()
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .unwrap()
        .source
        .unwrap();
    candidate
        .editor_mut()
        .apply(
            receipt.revision.document,
            NativeEditOwner::Human,
            Edit::RenameClip,
            &[NativeEdit::Clip(NativeClipEdit::Rename {
                source,
                clip,
                name: "Kept take".into(),
            })],
        )
        .unwrap();
    let checkpoint = candidate.files().create_checkpoint("recorded").unwrap();
    session
        .prepare_native_transition(&mut candidate, &NoPlugins, graph_limits())
        .unwrap();
    assert!(candidate.missing_audio().is_empty());
    let original_audio = samples(&mut candidate);
    assert!(
        original_audio
            .channel(0)
            .iter()
            .any(|sample| sample.abs() > 0.01)
    );

    let revision = candidate.editor().revision().document;
    candidate
        .editor_mut()
        .apply(
            revision,
            NativeEditOwner::Human,
            Edit::DeleteClip,
            &[NativeEdit::Clip(NativeClipEdit::Remove { source, clip })],
        )
        .unwrap();
    session
        .prepare_native_transition(&mut candidate, &NoPlugins, graph_limits())
        .unwrap();
    assert!(
        samples(&mut candidate)
            .channel(0)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    let revision = candidate.editor().revision().document;
    candidate
        .files_mut()
        .restore_checkpoint(&checkpoint, revision, NativeEditOwner::Human)
        .unwrap();
    let restored = candidate.editor().document().clone();
    let revision = candidate.editor().revision().document;
    candidate.editor_mut().undo(revision).unwrap();
    let revision = candidate.editor().revision().document;
    candidate.editor_mut().redo(revision).unwrap();
    assert_eq!(candidate.editor().document(), &restored);

    let job = candidate
        .files_mut()
        .begin_save_as(&directory.path().join("Native.auris"))
        .unwrap();
    candidate
        .files_mut()
        .finish_save(job.run().unwrap())
        .unwrap();
    let native_path = candidate.files().path().unwrap().to_path_buf();
    assert_eq!(
        std::fs::read(native_path.parent().unwrap().join("Audio/take.wav")).unwrap(),
        std::fs::read(&wav).unwrap()
    );
    assert!(!candidate.editor().is_dirty());
    let mut reopened = session
        .open_native_transition(&native_path, limits)
        .unwrap();
    assert_eq!(reopened.editor().document(), &restored);
    session
        .prepare_native_transition(&mut reopened, &NoPlugins, graph_limits())
        .unwrap();
    let reopened_audio = samples(&mut reopened);
    assert_eq!(reopened_audio.channel(0), original_audio.channel(0));
    assert_eq!(reopened_audio.channel(1), original_audio.channel(1));
    assert_eq!(session.project(), &original);
    assert_eq!(std::fs::read(original_path).unwrap(), original_bytes);
    assert_eq!(session.engine.adopted_native_generation(), None);
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
