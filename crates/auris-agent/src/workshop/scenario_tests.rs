//! End-to-end workshop scenarios with scripted providers and real persisted scores.

use super::*;
use std::path::Path;

struct Running {
    commands: mpsc::Sender<Command>,
    snapshots: mpsc::Receiver<Snapshot>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    fn start(store: Store) -> Self {
        let (events, snapshots) = mpsc::channel();
        Self::with_owner(Owner::new(store, Settings::default(), events), snapshots)
    }

    fn with_owner(mut owner: Owner, snapshots: mpsc::Receiver<Snapshot>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || owner.run(receiver, stop));
        Self {
            commands,
            snapshots,
            stopped,
            thread: Some(thread),
        }
    }

    fn until(&self, ready: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = None;
        loop {
            match self
                .snapshots
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(snapshot) => {
                    if ready(&snapshot) {
                        return snapshot;
                    }
                    last = Some(snapshot);
                }
                Err(error) => panic!(
                    "Workshop scenario timed out: {error}; last document={}, error={:?}",
                    last.as_ref().map(|s| json!(s.document)).unwrap_or_default(),
                    last.as_ref().and_then(|s| s.error.as_deref())
                ),
            }
        }
    }

    fn finished(&self, rounds: usize) -> Snapshot {
        self.until(|s| s.document.rounds.len() == rounds && !s.planning && s.progress.is_empty())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn candidate(id: u64, parent: Option<u64>, ready: bool) -> Candidate {
    Candidate {
        id,
        parent,
        ready,
        direction: Direction {
            title: format!("Song {id}"),
            description: "Two-bar drum arrangement".into(),
            instructions: "Preserve the notes and apply only the requested timeline edit.".into(),
            protected_tracks: vec![1],
            sources: vec![],
            phase: None,
        },
        error: None,
        recoveries: 0,
        tools: Default::default(),
        feedback: None,
    }
}

fn seed(store: &Store, selected: Option<u64>) {
    let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
    let cancel = AtomicBool::new(false);
    execute(
        &mut session,
        json!({"action":"add_track","name":"Original drums","kind":"drum"}),
        None,
        &Settings::default(),
        &cancel,
    )
    .unwrap();
    execute(&mut session, json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}), None, &Settings::default(), &cancel).unwrap();
    let first = candidate(1, None, true);
    session
        .finish_workshop_candidate(&store.candidate_path(1), idea(&first.direction), &cancel)
        .unwrap();
    store
        .save(&Document {
            next_id: 1,
            selected,
            rounds: vec![Round {
                after_message: 0,
                candidates: vec![first],
            }],
            ..Default::default()
        })
        .unwrap();
}

fn director() -> String {
    crate::tests::completion(
        &json!({"role":"assistant","content":json!({
        "reply":"Apply the requested edit while preserving the notes.",
        "directions":[candidate(0, None, false).direction]
    }).to_string()})
        .to_string(),
        "stop",
    )
}

fn edit(operation: serde_json::Value) -> String {
    crate::tests::completion(
        &json!({"role":"assistant","tool_calls":[{
            "id":"timeline-edit", "type":"function", "function":{
                "name":"edit_timeline", "arguments":json!({"operation":operation}).to_string()
            }
        }]})
        .to_string(),
        "tool_calls",
    )
}

fn answer() -> String {
    crate::tests::completion(
        &json!({"role":"assistant","content":"Finished the requested edit."}).to_string(),
        "stop",
    )
}

fn prefs(url: String) -> AgentPreferences {
    AgentPreferences {
        provider: "openai".into(),
        model: "mock".into(),
        url,
        ..Default::default()
    }
}

fn open_score(store: &Store, id: u64) -> Session {
    let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
    assert!(
        session
            .open(&store.candidate_path(id).join("song/song.auris"))
            .unwrap()
            .is_empty()
    );
    session
}

fn song_bytes(store: &Store, id: u64) -> Vec<u8> {
    std::fs::read(store.candidate_path(id).join("song/song.auris")).unwrap()
}

fn rendered_copy(store: &Store, id: u64, bpm: f64) -> Arc<SongCandidate> {
    let mut session = open_score(store, 1);
    session.set_tempo_at(auris_session::prelude::Ticks::ZERO, bpm);
    Arc::new(
        session
            .finish_workshop_candidate(
                &store.candidate_path(id),
                idea(&candidate(id, Some(1), false).direction),
                &AtomicBool::new(false),
            )
            .unwrap(),
    )
}

#[test]
fn unselected_song_revisions_accumulate_across_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, None);
    let (url, _) = crate::tests::mock_server(vec![
        director(),
        edit(json!({"kind":"set_tempo","at_tick":0,"bpm":90.0})),
        answer(),
        director(),
        edit(json!({"kind":"set_meter","at_tick":0,"numerator":3,"denominator":4})),
        answer(),
    ]);
    let prefs = prefs(url);
    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Send(
            "Slow this song to 90 BPM.".into(),
            Box::new(prefs.clone()),
            vec![],
        ))
        .unwrap();
    assert!(running.finished(2).document.rounds[1].candidates[0].ready);
    drop(running);
    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Send(
            "Keep 90 BPM and change the meter to 3/4.".into(),
            Box::new(prefs),
            vec![],
        ))
        .unwrap();
    let finished = running.finished(3);
    assert!(finished.document.rounds[2].candidates[0].ready);
    assert_eq!(finished.document.rounds[2].candidates[0].parent, Some(2));
    assert_eq!(open_score(&store, 3).project().bpm(), 90.0);
    assert_eq!(finished.document.selected, None);
}

#[test]
fn comparison_results_can_finish_in_reverse_order_without_selecting_a_winner() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    let second = rendered_copy(&store, 2, 80.0);
    let third = rendered_copy(&store, 3, 100.0);
    let mut document = store.load().unwrap();
    document.next_id = 3;
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![candidate(2, Some(1), false), candidate(3, Some(1), false)],
    });
    store.save(&document).unwrap();
    let (events, snapshots) = mpsc::channel();
    let mut owner = Owner::new(store.clone(), Settings::default(), events);
    for id in [2, 3] {
        owner.jobs.insert(id, Arc::new(AtomicBool::new(false)));
        owner.snapshot.progress.insert(id, "render".into());
    }
    owner.job_sender.send(JobEvent::Done(3, Ok(third))).unwrap();
    owner
        .job_sender
        .send(JobEvent::Done(2, Ok(second)))
        .unwrap();
    let running = Running::with_owner(owner, snapshots);
    let finished = running.finished(2);
    assert!(
        finished.document.rounds[1]
            .candidates
            .iter()
            .all(|c| c.ready)
    );
    assert_eq!(finished.document.selected, Some(1));
    assert_eq!(finished.document.continuation_candidate().unwrap().id, 1);
    assert_eq!(open_score(&store, 2).project().bpm(), 80.0);
    assert_eq!(open_score(&store, 3).project().bpm(), 100.0);
}

#[test]
fn stopping_a_candidate_rejects_an_already_queued_completion() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    let original = song_bytes(&store, 1);
    let second = rendered_copy(&store, 2, 90.0);
    let mut document = store.load().unwrap();
    document.next_id = 2;
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![candidate(2, Some(1), false)],
    });
    store.save(&document).unwrap();
    let (events, snapshots) = mpsc::channel();
    let mut owner = Owner::new(store.clone(), Settings::default(), events);
    owner.jobs.insert(2, Arc::new(AtomicBool::new(false)));
    owner.snapshot.progress.insert(2, "render".into());
    owner
        .job_sender
        .send(JobEvent::Done(2, Ok(second)))
        .unwrap();
    owner.command(Command::Stop(Some(2)));
    let running = Running::with_owner(owner, snapshots);
    let finished = running.finished(2);
    let stopped = &finished.document.rounds[1].candidates[0];
    assert!(!stopped.ready);
    assert_eq!(stopped.error.as_deref(), Some("Composition stopped"));
    assert_eq!(finished.document.selected, Some(1));
    assert!(!finished.candidates.contains_key(&2));
    assert_eq!(song_bytes(&store, 1), original);
}

#[test]
fn explicitly_returning_to_an_older_song_starts_a_separate_branch() {
    const NAME: &str =
        "workshop::scenario_tests::explicitly_returning_to_an_older_song_starts_a_separate_branch";
    if std::env::var_os("AURIS_WORKSHOP_BRANCH_TEST_CHILD").is_none() {
        let config = tempfile::tempdir().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env("AURIS_WORKSHOP_BRANCH_TEST_CHILD", "1")
            .env(auris_session::CONFIG_DIR_VAR, config.path())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    rendered_copy(&store, 2, 90.0);
    let mut document = store.load().unwrap();
    document.selected = Some(2);
    document.next_id = 2;
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![candidate(2, Some(1), true)],
    });
    store.save(&document).unwrap();
    let acknowledgement = crate::tests::completion(&json!({"role":"assistant","content":json!({"reply":"Continue from the original song.","directions":[]}).to_string()}).to_string(), "stop");
    let (url, _) = crate::tests::mock_server(vec![
        acknowledgement,
        director(),
        edit(json!({"kind":"set_meter","at_tick":0,"numerator":3,"denominator":4})),
        answer(),
    ]);
    let prefs = prefs(url);
    let running = Running::start(store.clone());
    // The desktop loads earlier cards before enabling their selection buttons.
    running.commands.send(Command::View(0)).unwrap();
    running.until(|s| s.candidates.contains_key(&1));
    running
        .commands
        .send(Command::Select(
            1,
            Box::new(prefs.clone()),
            "Return to the original 120 BPM song.".into(),
        ))
        .unwrap();
    let selected = running.until(|s| s.document.messages.len() == 2 && !s.planning);
    assert_eq!(selected.document.selected, Some(1));
    running
        .commands
        .send(Command::Send(
            "Make this original version 3/4, without changing tempo or notes.".into(),
            Box::new(prefs),
            vec![],
        ))
        .unwrap();
    let finished = running.finished(3);
    assert!(finished.document.rounds[2].candidates[0].ready);
    assert_eq!(finished.document.rounds[2].candidates[0].parent, Some(1));
    assert_eq!(open_score(&store, 3).project().bpm(), 120.0);
    assert_eq!(open_score(&store, 2).project().bpm(), 90.0);
}

fn real_round(running: &Running, rounds: usize, output: &Path) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(360);
    let mut saw_planning = false;
    loop {
        let snapshot = running
            .snapshots
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("real workshop round timed out; inspect state.json");
        std::fs::write(output.join("state.json"), serde_json::to_vec_pretty(&json!({
            "document":snapshot.document,"progress":snapshot.progress,"planning":snapshot.planning,"error":snapshot.error,
        })).unwrap()).unwrap();
        assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        saw_planning |= snapshot.planning;
        if saw_planning && !snapshot.planning && snapshot.progress.is_empty() {
            assert_eq!(
                snapshot.document.rounds.len(),
                rounds,
                "the real director did not dispatch the requested music"
            );
        }
        if snapshot.document.rounds.len() == rounds
            && !snapshot.planning
            && snapshot.progress.is_empty()
        {
            for candidate in &snapshot.document.rounds.last().unwrap().candidates {
                assert!(
                    candidate.ready,
                    "Real candidate {} failed: {:?}",
                    candidate.id, candidate.error
                );
            }
            return snapshot;
        }
    }
}

#[test]
#[ignore = "real signed-in model; requires AURIS_WORKSHOP_SCENARIO_OUTPUT and AURIS_WORKSHOP_EVAL_MODEL"]
fn installed_workshop_handles_sequential_edits_comparisons_and_thread_switching() {
    let output = PathBuf::from(
        std::env::var_os("AURIS_WORKSHOP_SCENARIO_OUTPUT")
            .expect("fresh scenario output directory"),
    );
    const CHILD: &str = "AURIS_WORKSHOP_SCENARIO_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(output.is_absolute() && !output.exists());
        std::fs::create_dir_all(&output).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workshop::scenario_tests::installed_workshop_handles_sequential_edits_comparisons_and_thread_switching", "--ignored", "--nocapture"])
            .env(CHILD, "1").env(auris_session::CONFIG_DIR_VAR, output.join("config"))
            .output().unwrap();
        std::fs::write(output.join("stdout.log"), &child.stdout).unwrap();
        std::fs::write(output.join("stderr.log"), &child.stderr).unwrap();
        assert!(
            child.status.success(),
            "See {}: {}",
            output.display(),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
    let prefs = AgentPreferences {
        backend: std::env::var("AURIS_WORKSHOP_EVAL_BACKEND").unwrap_or_else(|_| "codex".into()),
        model: std::env::var("AURIS_WORKSHOP_EVAL_MODEL").expect("explicit real model"),
        max_tool_calls: Some(80),
        ..Default::default()
    };
    let store = Store::new(output.join("conversations"));
    seed(&store, Some(1));
    let original = song_bytes(&store, 1);
    let running = Running::start(store.clone());
    running.commands.send(Command::Send("今の曲のテンポだけを90 BPMにしてください。音符、トラック、音色、ミックスは変更しないでください。候補は1案だけ。新規作曲は不要です。".into(), Box::new(prefs.clone()), vec![])).unwrap();
    let first = real_round(&running, 2, &output);
    let first_id = first.document.rounds[1].candidates[0].id;
    assert_eq!(open_score(&store, first_id).project().bpm(), 90.0);
    drop(running);

    let running = Running::start(store.clone());
    running.commands.send(Command::Send("先ほど変更した90 BPMを保ったまま、拍子だけを3/4にしてください。音符、トラック、音色、ミックスは変更しないでください。候補は1案だけ。".into(), Box::new(prefs.clone()), vec![])).unwrap();
    let second = real_round(&running, 3, &output);
    let second_id = second.document.rounds[2].candidates[0].id;
    let revised = open_score(&store, second_id);
    assert_eq!(revised.project().bpm(), 90.0);
    assert_eq!(
        revised
            .project()
            .signatures
            .signature_at(auris_session::prelude::Ticks::ZERO)
            .numerator,
        3
    );
    assert_eq!(
        revised.project().tracks,
        open_score(&store, 1).project().tracks
    );
    assert_eq!(
        second.document.rounds[2].candidates[0].parent,
        Some(first_id)
    );
    let revised_bytes = song_bytes(&store, second_id);

    running.commands.send(Command::Send("この3/4版から、テンポだけ80 BPMにする案と100 BPMにする案の2案を作って比較したいです。音符、トラック、音色、拍子、ミックスは変更しないでください。90 BPM版も残してください。".into(), Box::new(prefs), vec![])).unwrap();
    let comparison = real_round(&running, 4, &output);
    assert_eq!(comparison.document.rounds[3].candidates.len(), 2);
    let mut tempos = Vec::new();
    for candidate in &comparison.document.rounds[3].candidates {
        assert_eq!(candidate.parent, Some(second_id));
        let song = open_score(&store, candidate.id);
        assert_eq!(song.project().tracks, revised.project().tracks);
        assert_eq!(song.project().signatures, revised.project().signatures);
        tempos.push(song.project().bpm() as u32);
    }
    tempos.sort_unstable();
    assert_eq!(tempos, vec![80, 100]);
    assert_eq!(comparison.document.selected, Some(second_id));
    assert_eq!(song_bytes(&store, 1), original);
    assert_eq!(song_bytes(&store, second_id), revised_bytes);

    let thread_id = comparison.document.thread_id.clone();
    running.commands.send(Command::NewThread).unwrap();
    let empty = running.until(|s| s.document.thread_id != thread_id);
    assert!(
        empty.document.rounds.is_empty()
            && empty.document.messages.is_empty()
            && empty.candidates.is_empty()
    );
    let new_store = store.thread(&empty.document.thread_id).unwrap();
    let fresh_prefs = AgentPreferences {
        backend: std::env::var("AURIS_WORKSHOP_EVAL_BACKEND").unwrap_or_else(|_| "codex".into()),
        model: std::env::var("AURIS_WORKSHOP_EVAL_MODEL").unwrap(),
        max_tool_calls: Some(80),
        ..Default::default()
    };
    running.commands.send(Command::Send("新しいオリジナルの短いドラム曲を1案作ってください。100 BPM、4/4、2小節だけ。内蔵ドラム音源のみ、歌なし。他の会話の曲は使わず、新規に作ってください。".into(), Box::new(fresh_prefs), vec![])).unwrap();
    let fresh = real_round(&running, 1, &output);
    let fresh_id = fresh.document.rounds[0].candidates[0].id;
    assert_eq!(fresh.document.rounds[0].candidates[0].parent, None);
    let fresh_song = open_score(&new_store, fresh_id);
    assert_eq!(fresh_song.project().bpm(), 100.0);
    assert!(!fresh_song.project().tracks.is_empty());
    running
        .commands
        .send(Command::OpenThread(thread_id.clone()))
        .unwrap();
    let restored = running.until(|s| s.document.thread_id == thread_id);
    assert_eq!(restored.document.rounds.len(), 4);
    assert_eq!(restored.document.selected, Some(second_id));
    assert!(restored.candidates.contains_key(&second_id));
    std::fs::write(output.join("report.json"), serde_json::to_vec_pretty(&json!({
        "status":"passed", "model":std::env::var("AURIS_WORKSHOP_EVAL_MODEL").unwrap(),
        "first_edit":first_id,"second_edit":second_id,"comparison_tempos":tempos,
        "source_files_unchanged":true,"protected_notes_unchanged":true,"thread_switch_restored":true,
        "fresh_song_in_new_thread":true,
        "listening":"unassessed"
    })).unwrap()).unwrap();
}

#[test]
fn failed_revision_resumes_its_checkpoint_without_losing_prior_edits() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    let original = song_bytes(&store, 1);
    let mut interrupted = open_score(&store, 1);
    interrupted.set_tempo_at(auris_session::prelude::Ticks::ZERO, 90.0);
    save_checkpoint(&mut interrupted, &store.candidate_path(2)).unwrap();
    drop(interrupted);
    let mut failed = candidate(2, Some(1), false);
    failed.error = Some("Connection lost after changing the tempo".into());
    let mut document = store.load().unwrap();
    document.next_id = 2;
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![failed],
    });
    store.save(&document).unwrap();
    let (url, _) = crate::tests::mock_server(vec![
        edit(json!({"kind":"set_meter","at_tick":0,"numerator":3,"denominator":4})),
        answer(),
    ]);
    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Recover(2, Box::new(prefs(url))))
        .unwrap();
    let finished = running.until(|s| s.document.rounds[1].candidates[0].ready);
    assert_eq!(finished.document.rounds.len(), 2);
    assert_eq!(open_score(&store, 2).project().bpm(), 90.0);
    assert_eq!(
        open_score(&store, 2).project().tracks,
        open_score(&store, 1).project().tracks
    );
    assert_eq!(song_bytes(&store, 1), original);
}

#[test]
fn finishing_an_older_recovery_keeps_the_newer_selected_song() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    let second = rendered_copy(&store, 2, 80.0);
    rendered_copy(&store, 3, 90.0);
    let mut document = store.load().unwrap();
    document.selected = Some(3);
    document.next_id = 3;
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![candidate(2, Some(1), false)],
    });
    document.rounds.push(Round {
        after_message: 0,
        candidates: vec![candidate(3, Some(1), true)],
    });
    store.save(&document).unwrap();
    let (events, snapshots) = mpsc::channel();
    let mut owner = Owner::new(store.clone(), Settings::default(), events);
    owner.jobs.insert(2, Arc::new(AtomicBool::new(false)));
    owner.snapshot.progress.insert(2, "render".into());
    owner
        .job_sender
        .send(JobEvent::Done(2, Ok(second)))
        .unwrap();
    let running = Running::with_owner(owner, snapshots);
    let finished = running.finished(3);
    assert!(finished.document.rounds[1].candidates[0].ready);
    assert_eq!(finished.document.selected, Some(3));
    assert_eq!(open_score(&store, 3).project().bpm(), 90.0);
}

#[test]
fn selected_song_revisions_accumulate_across_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    let original = song_bytes(&store, 1);
    let (url, _) = crate::tests::mock_server(vec![
        director(),
        edit(json!({"kind":"set_tempo","at_tick":0,"bpm":90.0})),
        answer(),
        director(),
        edit(json!({"kind":"set_meter","at_tick":0,"numerator":3,"denominator":4})),
        answer(),
    ]);
    let prefs = prefs(url);
    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Send(
            "Slow this song to 90 BPM, keeping its notes.".into(),
            Box::new(prefs.clone()),
            vec![],
        ))
        .unwrap();
    let first = running.finished(2);
    assert!(
        first.document.rounds[1].candidates[0].ready,
        "{:?}",
        first.document.rounds[1].candidates[0].error
    );
    assert_eq!(open_score(&store, 2).project().bpm(), 90.0);
    drop(running);

    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Send(
            "Now change it to 3/4; keep the new tempo and notes.".into(),
            Box::new(prefs),
            vec![],
        ))
        .unwrap();
    let second = running.finished(3);
    assert!(
        second.document.rounds[2].candidates[0].ready,
        "{:?}",
        second.document.rounds[2].candidates[0].error
    );
    let final_score = open_score(&store, 3);
    assert_eq!(
        final_score.project().bpm(),
        90.0,
        "the second revision must retain the first revision"
    );
    assert_eq!(second.document.rounds[2].candidates[0].parent, Some(2));
    assert_eq!(
        final_score.project().tracks,
        open_score(&store, 1).project().tracks
    );
    assert_eq!(song_bytes(&store, 1), original);
}

#[test]
fn a_composer_that_finishes_with_lint_warnings_is_sent_back_a_bounded_number_of_times() {
    use auris_session::prelude::{Note, Ticks};
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into());
    seed(&store, Some(1));
    // A duplicated note on a track the direction does not protect: a warning the composer
    // never fixes here.
    let mut parent = open_score(&store, 1);
    let keys = parent.add_default_instrument_track("Keys").unwrap();
    let clip = parent
        .add_midi_clip(keys, "Chords", Ticks::ZERO, Ticks(3840))
        .unwrap();
    let note = Note::new(60, Ticks::ZERO, Ticks(480));
    parent
        .replace_notes(clip, vec![note.clone(), note])
        .unwrap();
    parent
        .finish_workshop_candidate(
            &store.candidate_path(1),
            idea(&candidate(1, None, true).direction),
            &AtomicBool::new(false),
        )
        .unwrap();
    let (url, _) = crate::tests::mock_server(vec![
        director(),
        edit(json!({"kind":"set_tempo","at_tick":0,"bpm":90.0})),
        answer(),
        answer(),
        answer(),
    ]);
    let running = Running::start(store.clone());
    running
        .commands
        .send(Command::Send(
            "Slow this song to 90 BPM.".into(),
            Box::new(prefs(url)),
            vec![],
        ))
        .unwrap();
    let finished = running.finished(2);
    let made = &finished.document.rounds[1].candidates[0];
    assert!(made.ready, "{:?}", made.error);
    let log =
        std::fs::read_to_string(store.candidate_path(made.id).join(composer_log::FILE)).unwrap();
    let kinds: Vec<String> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|entry| entry["kind"].as_str().map(str::to_owned))
        .filter(|kind| kind == "send_back" || kind == "unresolved")
        .collect();
    assert_eq!(kinds, ["send_back", "send_back", "unresolved"]);
    assert!(log.contains("duplicate_note"));
    assert_eq!(open_score(&store, made.id).project().bpm(), 90.0);
}
