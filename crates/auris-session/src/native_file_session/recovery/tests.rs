use super::*;
use crate::native_edit::{NativeClipEdit, NativeEdit, NativeEditOwner};
use crate::native_file_session::NativeRecordingFile;
use crate::{Edit, Session, SessionOptions};
use auris_core::{AssetPath, Project, Ticks};

fn limits() -> ContentLimits {
    crate::native_edit::tests::limits()
}

fn files() -> NativeFileSession {
    let project = Project::new("Recovery", 48_000.0);
    let document = auris_io::project_to_native(&project, limits()).unwrap();
    NativeFileSession::from_editor(
        NativeDocumentEditor::new_unsaved(document, limits()).unwrap(),
        limits(),
        None,
    )
}

fn rename(files: &mut NativeFileSession, name: &str) {
    let revision = files.editor().revision().document;
    files
        .editor_mut()
        .apply(
            revision,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::Timeline(
                crate::native_edit::NativeTimelineEdit::SetName { name: name.into() },
            )],
        )
        .unwrap();
}

fn stage(files: &mut NativeFileSession, root: &Path) -> NativeAutosaveResult {
    files
        .begin_autosave_in(root.to_path_buf())
        .unwrap()
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap()
        .unwrap()
}

fn abandon(files: &mut NativeFileSession) {
    let mut state = files.recovery.take().unwrap();
    Arc::get_mut(&mut state.workspace)
        .unwrap()
        .keep_for_recovery();
}

#[test]
fn native_recovery_uses_fallback_when_primary_registry_is_unwritable() {
    let temp = tempfile::tempdir().unwrap();
    let blocked = temp.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    let fallback = temp.path().join("fallback");
    let mut files = files();
    let result = files
        .begin_autosave_in_roots(vec![blocked, fallback.clone()])
        .unwrap()
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap()
        .unwrap();
    files
        .finish_autosave(result, &AtomicBool::new(false))
        .unwrap();
    assert!(files.autosave_path().unwrap().starts_with(&fallback));
    abandon(&mut files);
    let snapshots = snapshots_in(&fallback).unwrap();
    assert_eq!(snapshots.len(), 1);
    NativeFileSession::discard_recovery(&snapshots[0]).unwrap();
}

#[test]
fn native_autosave_requires_idle_owner_and_keeps_dirty_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut files = files();
    let result = stage(&mut files, temp.path());
    files
        .finish_autosave(result, &AtomicBool::new(false))
        .unwrap();
    assert!(files.editor().is_dirty());
    assert!(
        files
            .begin_autosave_in(temp.path().to_path_buf())
            .unwrap()
            .is_none()
    );
    assert!(snapshots_in(temp.path()).unwrap().is_empty());
    let revision = files.editor().revision().document;
    files
        .editor_mut()
        .begin_transaction(revision, NativeEditOwner::Human, Edit::ExternalChanges)
        .unwrap();
    assert!(matches!(
        files.begin_autosave(),
        Err(NativeFileSessionError::ActiveEdit)
    ));
}

#[test]
fn stale_cancelled_and_foreign_autosaves_cannot_replace_current_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let mut files = files();
    let first = stage(&mut files, temp.path());
    files
        .finish_autosave(first, &AtomicBool::new(false))
        .unwrap();
    let path = files.autosave_path().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    rename(&mut files, "second");
    let stale = stage(&mut files, temp.path());
    rename(&mut files, "third");
    assert!(matches!(
        files.finish_autosave(stale, &AtomicBool::new(false)),
        Err(NativeFileSessionError::StaleSave)
    ));
    let cancelled = stage(&mut files, temp.path());
    assert!(
        files
            .finish_autosave(cancelled, &AtomicBool::new(true))
            .is_err()
    );
    let foreign = stage(&mut files, temp.path());
    let mut other = NativeFileSession::new(files.editor().document().clone(), limits()).unwrap();
    assert!(
        other
            .finish_autosave(foreign, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(temp.path().read_dir().unwrap().count(), 1);
    assert_eq!(files.editor().document().context.name, "third");
}

#[test]
fn save_invalidates_pending_autosave_and_cleans_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let mut files = files();
    let result = stage(&mut files, &root);
    files
        .finish_autosave(result, &AtomicBool::new(false))
        .unwrap();
    let snapshot = files.autosave_path().unwrap();
    rename(&mut files, "changed");
    let late = stage(&mut files, &root);
    let save = files
        .begin_save_as(&temp.path().join("Song.auris"))
        .unwrap()
        .run()
        .unwrap();
    files.finish_save(save).unwrap();
    assert!(!files.editor().is_dirty());
    assert!(!snapshot.exists());
    assert!(
        files
            .finish_autosave(late, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(root.read_dir().unwrap().count(), 0);
}

#[test]
fn out_of_order_same_revision_jobs_and_cancelled_workers_leave_no_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let mut files = files();
    let first = stage(&mut files, temp.path());
    let second = stage(&mut files, temp.path());
    assert!(
        files
            .finish_autosave(first, &AtomicBool::new(false))
            .is_err()
    );
    files
        .finish_autosave(second, &AtomicBool::new(false))
        .unwrap();
    rename(&mut files, "changed");
    let job = files
        .begin_autosave_in(temp.path().to_path_buf())
        .unwrap()
        .unwrap();
    assert!(job.run(&AtomicBool::new(true)).unwrap().is_none());
    assert_eq!(temp.path().read_dir().unwrap().count(), 1);
}

#[test]
fn unsaved_recording_survives_workspace_loss_recovery_edit_save_as_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("native-recovery");
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session.add_audio_track("Recorded");
    let folder = session.project_folder().unwrap().to_path_buf();
    std::fs::create_dir_all(folder.join("Audio")).unwrap();
    let mut recorder =
        auris_io::WavRecorder::create(&folder.join("Audio/take.wav"), 48_000.0, 1).unwrap();
    let pcm: Vec<_> = (0..8192).map(|n| (n as f32 * 0.05).sin() * 0.25).collect();
    recorder.write(&pcm).unwrap();
    recorder.finish().unwrap();
    let mut transition = session.capture_native_transition(limits()).unwrap();
    assert_eq!(transition.files().asset_folder(), Some(folder.as_path()));
    let files = transition.files_mut();
    files
        .land_recorded_files(
            0,
            NativeEditOwner::Human,
            &[NativeRecordingFile {
                track,
                path: AssetPath::inside("Audio/take.wav"),
                start: Ticks::ZERO,
                offset_frames: 1024,
                length_frames: Some(4096),
            }],
        )
        .unwrap();
    let captured = files.editor().document().clone();
    let original =
        crate::native_assets::prepare_native_audio_assets(&captured, Some(&folder), 48_000.0, None)
            .unwrap();
    let result = stage(files, &root);
    files
        .finish_autosave(result, &AtomicBool::new(false))
        .unwrap();
    abandon(files);
    drop(transition);
    drop(session);
    assert!(!folder.exists());
    let snapshots = snapshots_in(&root).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].project_name(),
        Some(captured.context.name.as_str())
    );
    let mut runtime = Session::new(SessionOptions::headless()).unwrap();
    let mut recovered_transition = runtime
        .recover_native_transition(&snapshots[0], limits())
        .unwrap();
    struct NoPlugins;
    impl auris_core::pipeline::content::CapabilityProvider for NoPlugins {
        fn plugin<'a>(
            &'a self,
            _: &auris_core::pipeline::content::Processor,
        ) -> Option<auris_core::pipeline::content::PluginCapabilities<'a>> {
            None
        }
    }
    runtime
        .prepare_native_transition(
            &mut recovered_transition,
            &NoPlugins,
            auris_core::pipeline::PipelineLimits {
                nodes: 512,
                edges: 1024,
                ports: 4096,
                parameter_keys: 256,
                text_bytes: 1 << 20,
            },
        )
        .unwrap();
    assert!(recovered_transition.prepared_generation().is_some());
    assert!(recovered_transition.missing_audio().is_empty());
    let recovered = recovered_transition.files_mut();
    assert!(recovered.path().is_none());
    assert!(recovered.editor().is_dirty());
    assert_eq!(recovered.editor().document(), &captured);
    let forbidden = recovered.asset_folder().unwrap().join("Transient.auris");
    assert!(
        recovered
            .begin_save_as(&forbidden)
            .err()
            .unwrap()
            .to_string()
            .contains("permanent project folder")
    );
    NativeFileSession::discard_recovery(&snapshots[0]).unwrap();
    let source = captured
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .unwrap()
        .source
        .unwrap();
    let auris_core::pipeline::content::SourceContent::Audio(source_clips) = &captured
        .content
        .sources
        .iter()
        .find(|s| s.id == source)
        .unwrap()
        .content
    else {
        panic!("audio source");
    };
    let clip = source_clips[0].id;
    recovered
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::RenameClip,
            &[NativeEdit::Clip(NativeClipEdit::Rename {
                source,
                clip,
                name: "Recovered take".into(),
            })],
        )
        .unwrap();
    recovered.editor_mut().undo(1).unwrap();
    recovered.editor_mut().redo(2).unwrap();
    let expected = recovered.editor().document().clone();
    let workspace = recovered.asset_folder().unwrap().to_path_buf();
    let save = recovered
        .begin_save_as(&temp.path().join("Recovered.auris"))
        .unwrap()
        .run()
        .unwrap();
    recovered.finish_save(save).unwrap();
    assert!(!workspace.exists());
    let reopened = NativeFileSession::open(recovered.path().unwrap(), limits()).unwrap();
    assert_eq!(reopened.editor().document(), &expected);
    let assets = crate::native_assets::prepare_native_audio_assets(
        reopened.editor().document(),
        reopened.asset_folder(),
        48_000.0,
        None,
    )
    .unwrap();
    assert!(assets.missing.is_empty());
    for id in expected.context.audio_sources.keys() {
        assert_eq!(
            original.bank.get(*id).unwrap().channel(0),
            assets.bank.get(*id).unwrap().channel(0)
        );
    }
    assert!(snapshots_in(&root).unwrap().is_empty());
}
