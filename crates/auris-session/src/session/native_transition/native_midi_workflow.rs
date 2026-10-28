//! Native MIDI append coverage through the same worker and file seams as the desktop workflow.

use super::*;
use crate::SessionOptions;
use crate::native_edit::NativeEditOwner;
use auris_core::pipeline::content::SourceContent;
use auris_core::project::{CurvePoint, Project};
use auris_core::{AudioBuffer, Note, Ticks};
use auris_engine::{Transport, render_block};
use std::sync::atomic::AtomicBool;

fn session() -> Session {
    Session::new(SessionOptions {
        shipped_fonts: false,
        shipped_dictionary: false,
        ..SessionOptions::headless()
    })
    .expect("headless session")
}

fn graph_limits() -> auris_core::pipeline::PipelineLimits {
    auris_core::pipeline::PipelineLimits {
        nodes: 512,
        edges: 1024,
        ports: 4096,
        parameter_keys: 256,
        text_bytes: 1 << 20,
    }
}

fn render(transition: &mut NativeSessionTransition, start_frame: u64) -> AudioBuffer {
    let graph = transition
        .hosts
        .as_mut()
        .expect("prepared native hosts")
        .prepared_graph_mut()
        .expect("prepared native graph");
    let mut buffer = AudioBuffer::new(2, 4096, 48_000.0);
    render_block(
        graph,
        &mut Transport::playing_from(start_frame),
        &mut buffer,
        true,
    );
    buffer
}

fn prepare_render(
    source: &mut Session,
    transition: &mut NativeSessionTransition,
    start_frame: u64,
) -> AudioBuffer {
    let provider =
        crate::native_conventional::tests::builtin_provider(transition.editor().document());
    source
        .prepare_native_transition(transition, &provider, graph_limits())
        .unwrap();
    render(transition, start_frame)
}

fn midi_fixture() -> Project {
    let mut project = Project::default();
    project.tempo_map.set_point(Ticks::ZERO, 96.0);
    let lead = project.add_instrument_track("Lead", auris_synth::Chiptune::ID);
    let clip = project
        .add_midi_clip(lead, "Lead phrase", Ticks::ZERO, Ticks::from_beats(6.0))
        .unwrap();
    let phrase = project.midi_clip_mut(clip).unwrap();
    phrase
        .notes
        .push(Note::new(60, Ticks::QUARTER, Ticks::QUARTER * 2));
    phrase
        .notes
        .push(Note::new(64, Ticks::QUARTER * 3, Ticks::QUARTER));
    phrase.bend = vec![
        CurvePoint {
            at: Ticks::ZERO,
            value: 0.0,
        },
        CurvePoint {
            at: Ticks::QUARTER,
            value: 2.0,
        },
        CurvePoint {
            at: Ticks::QUARTER * 2,
            value: 0.0,
        },
    ];
    phrase.controllers.insert(
        1,
        vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 0.0,
            },
            CurvePoint {
                at: Ticks::QUARTER,
                value: 1.0,
            },
            CurvePoint {
                at: Ticks::QUARTER * 5,
                value: 0.0,
            },
        ],
    );

    let drums = project.add_drum_track("Drums", "auris.synth.noisedrum");
    let drum_clip = project
        .add_midi_clip(drums, "Kit", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    project
        .midi_clip_mut(drum_clip)
        .unwrap()
        .notes
        .push(Note::new(36, Ticks::ZERO, Ticks::QUARTER));
    project
}

#[test]
fn native_midi_handoff_rejects_changed_or_foreign_ordinary_origin() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("import.mid");
    auris_io::write_midi_file(&path, &midi_fixture()).unwrap();
    let mut source = session();
    source.add_default_instrument_track("Original").unwrap();
    let mut transition = source
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let foreign = session();
    assert!(matches!(
        foreign.begin_native_midi_append(&transition, &path, Ticks::ZERO, NativeEditOwner::Human,),
        Err(NativeTransitionError::Stale)
    ));
    let job = source
        .begin_native_midi_append(&transition, &path, Ticks::ZERO, NativeEditOwner::Human)
        .unwrap();
    source
        .add_default_instrument_track("Edited during import")
        .unwrap();
    let before = transition.editor().document().clone();
    let result = job.run(&AtomicBool::new(false)).unwrap().unwrap();
    assert!(matches!(
        source.finish_native_midi_append(&mut transition, result),
        Err(NativeTransitionError::Stale)
    ));
    assert_eq!(transition.editor().document(), &before);
    assert!(!transition.editor().can_undo());
    assert!(matches!(
        source.begin_native_midi_append(&transition, &path, Ticks::ZERO, NativeEditOwner::Human,),
        Err(NativeTransitionError::Stale)
    ));
}

#[test]
fn ordinary_and_native_midi_append_preserve_performance_and_render_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let midi_path = directory.path().join("fixture.mid");
    let fixture = midi_fixture();
    std::fs::write(&midi_path, auris_io::write_midi_bytes(&fixture).unwrap()).unwrap();

    let mut ordinary = session();
    let ordinary_report = ordinary.import_midi(&midi_path).unwrap();
    assert_eq!(ordinary_report.notes, 3);
    assert_eq!(ordinary_report.tracks, 2);
    assert!(ordinary_report.length > Ticks::ZERO);
    let mut ordinary_shapes = Vec::new();
    let mut ordinary_starts = Vec::new();
    for track in &ordinary.project().tracks {
        if let Some(instrument) = track.kind.as_instrument() {
            for clip in &instrument.clips {
                ordinary_starts.push(clip.start);
                ordinary_shapes.push((
                    clip.notes.clone(),
                    clip.bend.clone(),
                    clip.controllers.clone(),
                    clip.length,
                ));
            }
        }
    }
    ordinary_shapes.sort_unstable_by_key(|shape| shape.3);
    ordinary_starts.sort_unstable();

    let mut source = session();
    let existing = source.add_default_instrument_track("Existing").unwrap();
    let source_document = source.project().clone();
    let before_tempo = source.project().tempo_map.clone();
    let before_signatures = source.project().signatures.clone();
    let mut transition = source
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    // Render at the first imported note (append offset + one quarter), using the actual source
    // tempo rather than a hard-coded frame conversion.
    let append_at = Ticks::QUARTER;
    let first_note_at = append_at + Ticks::QUARTER;
    let append_frame = source
        .project()
        .tempo_map
        .ticks_to_samples(first_note_at, source.project().sample_rate)
        .raw();
    let baseline = prepare_render(&mut source, &mut transition, append_frame);
    let job = source
        .begin_native_midi_append(&transition, &midi_path, append_at, NativeEditOwner::Human)
        .unwrap();
    let result = job
        .run(&AtomicBool::new(false))
        .unwrap()
        .expect("the MIDI import is not cancelled");
    let receipt = source
        .finish_native_midi_append(&mut transition, result)
        .unwrap();
    assert_eq!(source.project(), &source_document);
    assert!(transition.prepared_generation().is_none());
    assert_eq!(receipt.report, ordinary_report);
    assert!(receipt.edit.changed);
    assert_ne!(transition.editor().document().content.tracks.len(), 0);
    assert_eq!(source.project().tempo_map, before_tempo);
    assert_eq!(source.project().signatures, before_signatures);
    assert_eq!(
        transition.editor().document().context.tempo_map,
        before_tempo
    );
    assert_eq!(
        transition.editor().document().context.signatures,
        before_signatures
    );
    let mut native_shapes = Vec::new();
    let mut native_starts = Vec::new();
    for source_document in &transition.editor().document().content.sources {
        if let SourceContent::Notes(clips) = &source_document.content {
            for clip in clips {
                native_starts.push(clip.start);
                native_shapes.push((
                    clip.notes.clone(),
                    clip.bend.clone(),
                    clip.controllers.clone(),
                    clip.length,
                ));
            }
        }
    }
    native_shapes.sort_unstable_by_key(|shape| shape.3);
    assert_eq!(native_shapes, ordinary_shapes);
    native_starts.sort_unstable();
    assert_eq!(
        native_starts,
        ordinary_starts
            .iter()
            .map(|start| *start + append_at)
            .collect::<Vec<_>>()
    );
    assert!(
        transition
            .editor()
            .document()
            .content
            .tracks
            .iter()
            .any(|binding| binding.track != existing)
    );

    let appended = prepare_render(&mut source, &mut transition, append_frame);
    assert!(appended.channel(0).iter().any(|sample| sample.abs() > 1e-6));
    assert!(appended.channel(1).iter().any(|sample| sample.abs() > 1e-6));

    let revision = transition.editor().revision().document;
    transition.editor_mut().undo(revision).unwrap();
    let undone = prepare_render(&mut source, &mut transition, append_frame);
    assert_eq!(undone.channel(0), baseline.channel(0));
    assert_eq!(undone.channel(1), baseline.channel(1));
    let revision = transition.editor().revision().document;
    transition.editor_mut().redo(revision).unwrap();
    let redone = prepare_render(&mut source, &mut transition, append_frame);
    assert_eq!(redone.channel(0), appended.channel(0));
    assert_eq!(redone.channel(1), appended.channel(1));

    let chosen = directory.path().join("NativeMidi.auris");
    let job = transition.files_mut().begin_save_as(&chosen).unwrap();
    transition
        .files_mut()
        .finish_save(job.run().unwrap())
        .unwrap();
    let path = transition.files().path().unwrap().to_path_buf();
    let mut reopened = source
        .open_native_transition(&path, crate::native_edit::tests::limits())
        .unwrap();
    assert_eq!(reopened.editor().document(), transition.editor().document());
    let reopened_audio = prepare_render(&mut source, &mut reopened, append_frame);
    assert_eq!(reopened_audio.channel(0), appended.channel(0));
    assert_eq!(reopened_audio.channel(1), appended.channel(1));
    assert!(!reopened.editor().is_dirty());
}
