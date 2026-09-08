use super::*;
use crate::SessionOptions;
use crate::audio_evaluation::{AudioMetric, ReferenceAudioEvaluator};
use auris_core::{Note, Ticks};

fn session() -> Session {
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session.add_default_instrument_track("Lead").unwrap();
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(2.0))
        .unwrap();
    for (pitch, beat) in [(60, 0.0), (67, 1.0)] {
        session
            .add_note(
                clip,
                Note::new(pitch, Ticks::from_beats(beat), Ticks::QUARTER),
            )
            .unwrap();
    }
    session.project.track_mut(track).unwrap().mixer.gain_db = -12.0;
    // Unrelated transforms and their random identities must survive adoption exactly.
    session
        .project
        .track_mut(track)
        .unwrap()
        .kind
        .as_instrument_mut()
        .unwrap()
        .clips[0]
        .transforms
        .push(NoteTransform::Humanize {
            amount: 0.12,
            seed: 981,
        });
    session.forget_history();
    session
}

fn options() -> OfflineOptions {
    OfflineOptions {
        sample_rate: Some(SAMPLE_RATE),
        start_frames: 0,
        end_frames: Some(SAMPLE_RATE as u64),
        include_tail: false,
        ..OfflineOptions::default()
    }
}

fn render(session: &mut Session, project: &Project) -> AudioBuffer {
    let options = OfflineOptions {
        block_frames: session.engine.max_block(),
        ..options()
    };
    session
        .job_for(project.clone())
        .render_complete(&options, &mut RenderProgress::default())
        .unwrap()
}

fn settings() -> ReferenceMatchSettings {
    ReferenceMatchSettings {
        duration_seconds: 1.0,
        attempts: 5,
        ..ReferenceMatchSettings::default()
    }
}

fn finish(session: &mut Session, mut job: ReferenceMatchJob) -> ReferenceMatchReport {
    let mut fractions = Vec::new();
    loop {
        // Moving both halves proves the public worker boundary is Send.
        let (result, progress) = std::thread::spawn(move || {
            let mut progress = Vec::new();
            let result = job.run(&AtomicBool::new(false), &mut |f| progress.push(f));
            (result, progress)
        })
        .join()
        .unwrap();
        fractions.extend(progress);
        match session.continue_reference_match(result.unwrap()).unwrap() {
            ReferenceMatchStep::Pending(next) => job = next,
            ReferenceMatchStep::Complete(report) => {
                assert_eq!(fractions.last(), Some(&1.0));
                assert!(fractions.windows(2).all(|pair| pair[0] <= pair[1]));
                return report;
            }
        }
    }
}

fn target(session: &mut Session) -> (ReferenceMatchSettings, Arc<dyn AudioEvaluator>) {
    let original = session.project.clone();
    let mut settings = settings();
    settings.seed = (0..1000)
        .find(|&seed| {
            let trial = ReferenceMatchSettings {
                seed,
                ..settings.clone()
            };
            let dials = search_dials(&original, &trial);
            matches!(dials[0].control, Control::Pan) && matches!(dials[1].control, Control::Gate)
        })
        .expect("a seed orders pan then gate");
    let dials = search_dials(&original, &settings);
    let mut target = original.clone();
    dials[0].adjust(&mut target, &original, 1.0);
    dials[1].adjust(&mut target, &original, 1.0);
    let audio = render(session, &target);
    (
        settings,
        Arc::new(ReferenceAudioEvaluator::new(&audio).unwrap()),
    )
}

#[test]
fn rendered_reference_improves_mix_and_performance_and_applies_exactly_once() {
    let mut session = session();
    let (settings, evaluator) = target(&mut session);
    let original = session.project.clone();
    let baseline_audio = render(&mut session, &original);
    let job = session
        .begin_reference_match(settings.clone(), Arc::clone(&evaluator))
        .unwrap();
    let report = finish(&mut session, job);
    assert_eq!(
        session.project, original,
        "measurement changed the live document"
    );
    assert_eq!(report.baseline_audio.as_ref(), &baseline_audio);
    assert!(report.best.fitness > report.baseline.fitness);
    assert_eq!(report.attempts, settings.attempts);
    assert!(!report.cancelled);
    let before = &original.tracks[0];
    let after = &report.adoption.project.tracks[0];
    assert_ne!(before.mixer.pan, after.mixer.pan);
    let before_clip = &before.kind.as_instrument().unwrap().clips[0];
    let after_clip = &after.kind.as_instrument().unwrap().clips[0];
    assert_ne!(before_clip.transforms, after_clip.transforms);
    let mut restored_score = after_clip.clone();
    restored_score.transforms = before_clip.transforms.clone();
    assert_eq!(
        &restored_score, before_clip,
        "written notes, recipes, timing and other clip data changed"
    );
    assert_eq!(after_clip.transforms[0], before_clip.transforms[0]);
    assert_eq!(evaluator.evaluate(&report.best_audio).unwrap(), report.best);
    let exact = report.adoption.project.clone();
    assert_eq!(render(&mut session, &exact), *report.best_audio);
    assert!(session.apply_reference_match(&report).unwrap());
    assert_eq!(session.project, exact);
    assert_eq!(session.undo(), Some(Edit::MatchReference));
    assert_eq!(session.project, original);
    assert!(!session.can_undo());
    assert_eq!(session.redo(), Some(Edit::MatchReference));
    assert_eq!(session.project, exact);
    assert!(
        session.apply_reference_match(&report).is_err(),
        "a report cannot apply twice"
    );
}

#[test]
fn repeated_seed_retains_the_same_exact_audio_and_candidate() {
    let mut session = session();
    let (settings, evaluator) = target(&mut session);
    let first = session
        .begin_reference_match(settings.clone(), Arc::clone(&evaluator))
        .unwrap();
    let first = finish(&mut session, first);
    let second = session.begin_reference_match(settings, evaluator).unwrap();
    let second = finish(&mut session, second);
    assert_eq!(first.adoption.project, second.adoption.project);
    assert_eq!(first.best, second.best);
    assert_eq!(first.best_audio, second.best_audio);
}

struct Constant;
impl AudioEvaluator for Constant {
    fn evaluate(&self, _: &AudioBuffer) -> Result<AudioEvaluation, String> {
        Ok(AudioEvaluation {
            fitness: 1.0,
            metrics: Vec::new(),
        })
    }
    fn description(&self) -> String {
        "constant test objective".into()
    }
}

#[test]
fn ties_and_cancellation_retain_the_unchanged_baseline_without_history() {
    let mut session = session();
    let before = session.project.clone();
    let job = session
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    let ReferenceMatchStep::Pending(job) = session.continue_reference_match(result).unwrap() else {
        panic!("candidate pending")
    };
    let result = job.run(&AtomicBool::new(true), &mut |_| {}).unwrap();
    let ReferenceMatchStep::Complete(report) = session.continue_reference_match(result).unwrap()
    else {
        panic!("cancelled report complete")
    };
    assert!(report.cancelled);
    assert_eq!(report.attempts, 1);
    assert!(Arc::ptr_eq(&report.baseline_audio, &report.best_audio));
    assert_eq!(report.adoption.project, before);
    assert!(!session.apply_reference_match(&report).unwrap());
    assert!(!session.can_undo());
    let job = session
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    assert!(
        job.run(&AtomicBool::new(true), &mut |_| {})
            .is_err_and(|error| error.is_cancellation())
    );
    let job = session
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let report = finish(&mut session, job);
    assert_eq!(report.adoption.project, before);
    assert_eq!(report.baseline, report.best);
    assert!(report.changes.is_empty());
}

#[test]
fn edits_other_sessions_folders_and_undo_invalidate_results() {
    let mut source = session();
    let mut other = session();
    let job = source
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(other.continue_reference_match(result).is_err());
    let job = source
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    source
        .rename_track(source.project.tracks[0].id, "Edited")
        .unwrap();
    let edited = source.project.clone();
    assert!(source.continue_reference_match(result).is_err());
    assert_eq!(source.project, edited);
    let job = source
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let mut report = finish(&mut source, job);
    report.adoption.provenance.folder = Some(PathBuf::from("another-project"));
    assert!(source.apply_reference_match(&report).is_err());
    let job = source
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    let report = finish(&mut source, job);
    source.undo();
    assert!(source.apply_reference_match(&report).is_err());
}

struct Invalid;
impl AudioEvaluator for Invalid {
    fn evaluate(&self, _: &AudioBuffer) -> Result<AudioEvaluation, String> {
        Ok(AudioEvaluation {
            fitness: 1.0,
            metrics: vec![AudioMetric {
                name: "invalid".into(),
                value: f64::NAN,
            }],
        })
    }
    fn description(&self) -> String {
        "invalid test objective".into()
    }
}

#[test]
fn invalid_evaluators_settings_and_missing_render_sources_leave_document_intact() {
    let mut session = session();
    let before = session.project.clone();
    let job = session
        .begin_reference_match(settings(), Arc::new(Invalid))
        .unwrap();
    assert!(job.run(&AtomicBool::new(false), &mut |_| {}).is_err());
    for duration_seconds in [f64::NAN, 0.5, 31.0] {
        assert!(
            session
                .begin_reference_match(
                    ReferenceMatchSettings {
                        duration_seconds,
                        ..settings()
                    },
                    Arc::new(Constant)
                )
                .is_err()
        );
    }
    assert_eq!(session.project, before);
    assert!(!session.can_undo());
    session
        .project
        .master
        .effects
        .push(auris_core::EffectSlot::new(
            auris_core::EffectSlotId(99),
            "missing.effect",
        ));
    assert!(
        session
            .begin_reference_match(settings(), Arc::new(Constant))
            .is_err()
    );
    session.project = before.clone();
    session.project.tracks[0]
        .kind
        .as_instrument_mut()
        .unwrap()
        .instrument_id = auris_sampler::SAMPLER_ID.into();
    assert!(
        session
            .begin_reference_match(settings(), Arc::new(Constant))
            .is_err()
    );
    session.project = before;
    let audio = session.project.add_audio_track("Missing audio");
    let source = session.project.add_audio_source(
        "Missing",
        auris_core::AssetPath::external(PathBuf::from("missing.wav")),
        44_100,
        44_100.0,
        2,
    );
    session
        .project
        .add_audio_clip(audio, source, Ticks::ZERO)
        .unwrap();
    assert!(
        session
            .begin_reference_match(settings(), Arc::new(Constant))
            .is_err()
    );
    assert!(!session.can_undo());
}

#[test]
fn every_coordinate_remains_bounded_and_preserves_written_notes() {
    let session = session();
    let original = session.project.clone();
    let mut candidate = original.clone();
    for dial in search_dials(&original, &settings()) {
        for _ in 0..40 {
            dial.adjust(&mut candidate, &original, 1.0);
        }
    }
    let before = &original.tracks[0];
    let after = &candidate.tracks[0];
    assert!((before.mixer.gain_db - after.mixer.gain_db).abs() <= 3.0);
    assert!((before.mixer.pan - after.mixer.pan).abs() <= 0.3);
    let before = &before.kind.as_instrument().unwrap().clips[0];
    let after = &after.kind.as_instrument().unwrap().clips[0];
    assert_eq!(before.notes, after.notes);
    assert_eq!(before.recipe, after.recipe);
    assert_eq!(before.transforms[0], after.transforms[0]);
    let expression = expression(after);
    assert!(expression.timing <= 0.2 && expression.velocity <= 0.2 && expression.swell <= 0.2);
    assert!(expression.accent.abs() <= 0.3 && expression.delay_ms.abs() <= 8.0);
    assert!((gate(before) - gate(after)).abs() <= 0.100_001);
}

#[test]
fn native_edits_during_render_setup_cannot_be_rebased_into_the_old_provenance() {
    use super::super::hosted::{change_fixture_instrument_level, install_fixture_instrument};

    let mut session = session();
    let track = session.project.tracks[0].id;
    install_fixture_instrument(&mut session, track);
    session.forget_history();
    let ReferenceMatchJob { state, render } = session
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    drop(render);
    let original = session.project.clone();
    let revision = session.revision;
    // Simulate a native editor callback arriving after the continuation's guard and before
    // setup is polled. The plugin has changed its actual sound, but Project has not caught up.
    change_fixture_instrument_level(&mut session, track, 0.25);
    assert_eq!(session.project, original);
    assert_eq!(session.revision, revision);
    assert!(session.reference_job_for(state).is_err());
    assert_ne!(session.revision, revision);
    assert_eq!(
        session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state
            .hosted_bytes(),
        Some(0.25f32.to_le_bytes().to_vec()),
        "the native edit must be retained rather than overwritten by a candidate"
    );
    assert_eq!(session.project.tracks[0].mixer, original.tracks[0].mixer);
    assert!(
        !session.can_undo(),
        "a refused candidate must not add history"
    );
}

#[test]
fn unchanged_native_setup_is_accepted_but_a_later_native_edit_blocks_apply() {
    use super::super::hosted::{change_fixture_instrument_level, install_fixture_instrument};

    let mut session = session();
    let track = session.project.tracks[0].id;
    install_fixture_instrument(&mut session, track);
    session.forget_history();
    let ReferenceMatchJob { state, render } = session
        .begin_reference_match(settings(), Arc::new(Constant))
        .unwrap();
    drop(render);
    // A setup notification with byte-identical native state is safe to settle.
    change_fixture_instrument_level(&mut session, track, 1.0);
    let job = session.reference_job_for(state).unwrap();
    let report = finish(&mut session, job);
    change_fixture_instrument_level(&mut session, track, 0.5);
    assert!(session.apply_reference_match(&report).is_err());
    assert_eq!(
        session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state
            .hosted_bytes(),
        Some(0.5f32.to_le_bytes().to_vec()),
    );
    assert!(!session.can_undo());
}

#[test]
fn rendered_reference_search_does_not_reward_shared_gain_roundoff() {
    let mut session = session();
    let clip = &mut session.project.tracks[0]
        .kind
        .as_instrument_mut()
        .unwrap()
        .clips[0];
    // Match the twelve-second GUI smoke fixture, including its deterministic performance.
    clip.length = Ticks::from_beats(24.0);
    clip.notes = (0..24)
        .map(|beat| {
            Note::new(
                [60, 67, 64, 69, 65, 72, 67, 64][beat % 8],
                Ticks::from_beats(beat as f64),
                Ticks::QUARTER,
            )
        })
        .collect();
    let original = session.project.clone();
    let mut target = original.clone();
    target.tracks[0].mixer.pan = 0.15;
    target.tracks[0].kind.as_instrument_mut().unwrap().clips[0]
        .transforms
        .push(NoteTransform::Gate { amount: 0.95 });
    let options = OfflineOptions {
        end_frames: Some((12.0 * SAMPLE_RATE) as u64),
        block_frames: session.engine.max_block(),
        ..options()
    };
    let reference = session
        .job_for(target)
        .render_complete(&options, &mut RenderProgress::default())
        .unwrap();
    let evaluator = Arc::new(ReferenceAudioEvaluator::new(&reference).unwrap());
    let job = session
        .begin_reference_match(
            ReferenceMatchSettings {
                duration_seconds: 12.0,
                attempts: 32,
                seed: 42,
                ..settings()
            },
            evaluator,
        )
        .unwrap();
    let report = finish(&mut session, job);
    let winner = &report.adoption.project.tracks[0];
    assert!(report.best.fitness > report.baseline.fitness);
    assert_eq!(report.best.fitness, 0.0);
    assert_eq!(winner.mixer.pan, 0.15);
    assert_eq!(gate(&winner.kind.as_instrument().unwrap().clips[0]), 0.95);
    assert_eq!(
        winner.mixer.gain_db, original.tracks[0].mixer.gain_db,
        "uniform gain must not win through floating-point feature differences"
    );
    assert_eq!(session.project, original);
    assert!(!session.can_undo());
}
