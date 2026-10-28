//! Native plugin/effect edit coverage through the normal prepare and file seams.
//!
//! This module is included by `native_transition.rs` under `cfg(test)`.  Keeping the tests next
//! to the transition lets them render the prepared graph, which is deliberately not exposed as
//! a second mutable document to integration consumers.

use super::*;
use crate::native_edit::{NativeEdit, NativeEditOwner};
use crate::native_edit::{NativeEffectEdit, NativePluginEdit};
use crate::{Edit, SessionOptions};
use auris_core::pipeline::content::{PluginInstance, ProcessorKind};
use auris_core::{AudioBuffer, Note, PluginState, Ticks};
use auris_engine::{Transport, render_block};

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

fn render(transition: &mut NativeSessionTransition) -> AudioBuffer {
    let graph = transition
        .hosts
        .as_mut()
        .expect("prepared hosts")
        .prepared_graph_mut()
        .expect("prepared graph");
    let mut buffer = AudioBuffer::new(2, 4096, 48_000.0);
    render_block(graph, &mut Transport::playing_from(0), &mut buffer, true);
    buffer
}

fn energy(buffer: &AudioBuffer) -> f64 {
    buffer
        .channel(0)
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum()
}

fn prepare_render(source: &mut Session, transition: &mut NativeSessionTransition) -> AudioBuffer {
    let provider =
        crate::native_conventional::tests::builtin_provider(transition.editor().document());
    source
        .prepare_native_transition(transition, &provider, graph_limits())
        .unwrap();
    assert!(transition.prepared_generation().is_some());
    let buffer = render(transition);
    assert_eq!(buffer.channel_count(), 2);
    assert!(buffer.channel(0).iter().all(|sample| sample.is_finite()));
    assert!(buffer.channel(1).iter().all(|sample| sample.is_finite()));
    buffer
}

fn plugin(plugin_id: &str, params: &[(&str, f32)]) -> PluginInstance {
    let mut state = PluginState::default();
    state
        .params
        .extend(params.iter().map(|(key, value)| ((*key).into(), *value)));
    PluginInstance {
        plugin_id: plugin_id.into(),
        state,
        file: None,
    }
}

#[test]
fn native_plugin_edits_prepare_undo_redo_save_reopen_and_render() {
    let mut source = session();
    let track = source
        .add_instrument_track("Lead", auris_synth::Chiptune::ID)
        .unwrap();
    let clip = source
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    source
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    let slot = source.add_effect(Some(track), "auris.fx.gain").unwrap();
    let mut transition = source
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let baseline = prepare_render(&mut source, &mut transition);
    assert!(
        energy(&baseline) > 1e-8,
        "the fixture must render audible PCM"
    );

    let document = transition.editor().document();
    let instrument = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Instrument(_)))
        .unwrap()
        .id;
    let effect = document
        .content
        .processors
        .iter()
        .find(|processor| {
            matches!(&processor.kind, ProcessorKind::Effect { slot: id, .. } if *id == slot)
        })
        .unwrap()
        .id;

    let edits = [
        NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "waveform".into(),
            value: 2.0,
        }),
        NativeEdit::Plugin(NativePluginEdit::ReplaceState {
            node: instrument,
            state: {
                let mut state = PluginState::default();
                state.params.insert("waveform".into(), 3.0);
                state
            },
        }),
        NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
            node: instrument,
            plugin: plugin(auris_synth::Chiptune::ID, &[("waveform", 1.0)]),
        }),
        NativeEdit::Plugin(NativePluginEdit::ReplaceEffect {
            node: effect,
            plugin: plugin("auris.fx.gain", &[("gain_db", -12.0)]),
        }),
    ];
    let mut revision = transition.editor().revision().document;
    for (edit_index, edit) in edits.into_iter().enumerate() {
        let before = prepare_render(&mut source, &mut transition);
        transition
            .editor_mut()
            .apply(
                revision,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[edit],
            )
            .unwrap();
        revision = transition.editor().revision().document;
        assert!(transition.prepared_generation().is_none());
        let changed = prepare_render(&mut source, &mut transition);
        assert!(changed.channel(0).iter().all(|sample| sample.is_finite()));
        assert!(energy(&changed).is_finite());
        assert!(
            changed.channel(0) != before.channel(0),
            "plugin edit {edit_index} must change rendered PCM"
        );
        if edit_index == 3 {
            let expected = 10.0_f64.powf(-12.0 / 10.0);
            let ratio = energy(&changed) / energy(&before);
            assert!((ratio - expected).abs() < 0.01, "gain power ratio: {ratio}");
        }
        transition.editor_mut().undo(revision).unwrap();
        revision = transition.editor().revision().document;
        let undone = prepare_render(&mut source, &mut transition);
        assert_eq!(undone.channel(0), before.channel(0));
        assert_eq!(undone.channel(1), before.channel(1));
        transition.editor_mut().redo(revision).unwrap();
        revision = transition.editor().revision().document;
        let redone = prepare_render(&mut source, &mut transition);
        assert_eq!(redone.channel(0), changed.channel(0));
        assert_eq!(redone.channel(1), changed.channel(1));
    }
    let after = prepare_render(&mut source, &mut transition);
    assert!(after.channel(0).iter().all(|sample| sample.is_finite()));

    let folder = tempfile::tempdir().unwrap();
    let job = transition
        .files_mut()
        .begin_save_as(&folder.path().join("PluginEdits.auris"))
        .unwrap();
    transition
        .files_mut()
        .finish_save(job.run().unwrap())
        .unwrap();
    let path = transition.files().path().unwrap().to_path_buf();
    let expected = transition.editor().document().clone();
    let mut reopened = source
        .open_native_transition(&path, crate::native_edit::tests::limits())
        .unwrap();
    assert_eq!(reopened.editor().document(), &expected);
    let reopened_audio = prepare_render(&mut source, &mut reopened);
    assert_eq!(reopened_audio.channel(0), after.channel(0));
    assert_eq!(reopened_audio.channel(1), after.channel(1));
    assert!(!reopened.editor().is_dirty());
}

#[test]
fn native_effect_chain_edits_preserve_ids_and_round_trip_audio() {
    let mut source = session();
    let track = source
        .add_instrument_track("Lead", auris_synth::Chiptune::ID)
        .unwrap();
    let side = source
        .add_instrument_track("Side", auris_synth::Chiptune::ID)
        .unwrap();
    let clip = source
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    source
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    let first = source.add_effect(Some(track), "auris.fx.gain").unwrap();
    let second = source
        .add_effect(Some(track), "auris.fx.compressor")
        .unwrap();
    // Level before a compressor changes its envelope, so moving the gain after it
    // must change PCM even when the node payloads and identities stay unchanged.
    for (slot, plugin_id, key, value) in [
        (first, "auris.fx.gain", "gain_db", -12.0),
        (second, "auris.fx.compressor", "threshold_db", -30.0),
        (second, "auris.fx.compressor", "ratio", 8.0),
    ] {
        let param = source
            .param_descriptors(plugin_id)
            .iter()
            .find(|descriptor| descriptor.key == key)
            .unwrap()
            .id;
        source.set_param(
            auris_core::ParamTarget::Effect {
                track: Some(track),
                slot,
                param,
            },
            value,
        );
    }
    let mut transition = source
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    let baseline = prepare_render(&mut source, &mut transition);
    assert!(
        energy(&baseline) > 1e-8,
        "the fixture must render audible PCM"
    );
    let document = transition.editor().document();
    let group = document
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .unwrap()
        .group;
    let first_node = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Effect { slot, .. } if *slot == first))
        .unwrap()
        .id;
    let second_node = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Effect { slot, .. } if *slot == second))
        .unwrap()
        .id;
    let inserted = plugin("auris.fx.gain", &[("gain_db", -6.0)]);
    let mut revision = transition.editor().revision().document;
    for edit in [
        NativeEdit::Effect(NativeEffectEdit::Insert {
            group,
            index: 1,
            plugin: inserted,
        }),
        NativeEdit::Effect(NativeEffectEdit::Move {
            node: first_node,
            index: 2,
        }),
        NativeEdit::Effect(NativeEffectEdit::SetSidechain {
            node: second_node,
            source: Some(side),
        }),
    ] {
        let before = prepare_render(&mut source, &mut transition);
        transition
            .editor_mut()
            .apply(
                revision,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[edit],
            )
            .unwrap();
        revision = transition.editor().revision().document;
        assert!(transition.prepared_generation().is_none());
        let changed = prepare_render(&mut source, &mut transition);
        assert!(
            changed.channel(0) != before.channel(0),
            "insert order and sidechain changes must reach rendered PCM"
        );
        transition.editor_mut().undo(revision).unwrap();
        revision = transition.editor().revision().document;
        let undone = prepare_render(&mut source, &mut transition);
        assert_eq!(undone.channel(0), before.channel(0));
        assert_eq!(undone.channel(1), before.channel(1));
        transition.editor_mut().redo(revision).unwrap();
        revision = transition.editor().revision().document;
        let redone = prepare_render(&mut source, &mut transition);
        assert_eq!(redone.channel(0), changed.channel(0));
        assert_eq!(redone.channel(1), changed.channel(1));
    }
    let inserted_node = transition
        .editor()
        .document()
        .content
        .processors
        .iter()
        .find(|processor| {
            processor.id != first_node
                && processor.id != second_node
                && matches!(&processor.kind, ProcessorKind::Effect { .. })
        })
        .unwrap()
        .id;
    transition
        .editor_mut()
        .apply(
            revision,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::Effect(NativeEffectEdit::Remove {
                node: inserted_node,
            })],
        )
        .unwrap();
    let final_audio = prepare_render(&mut source, &mut transition);
    assert!(
        final_audio
            .channel(0)
            .iter()
            .all(|sample| sample.is_finite())
    );
    assert!(
        transition
            .editor()
            .document()
            .content
            .processors
            .iter()
            .any(|processor| processor.id == first_node)
    );

    let folder = tempfile::tempdir().unwrap();
    let job = transition
        .files_mut()
        .begin_save_as(&folder.path().join("EffectEdits.auris"))
        .unwrap();
    transition
        .files_mut()
        .finish_save(job.run().unwrap())
        .unwrap();
    let path = transition.files().path().unwrap().to_path_buf();
    let expected = transition.editor().document().clone();
    let mut reopened = source
        .open_native_transition(&path, crate::native_edit::tests::limits())
        .unwrap();
    assert_eq!(reopened.editor().document(), &expected);
    let reopened_audio = prepare_render(&mut source, &mut reopened);
    assert_eq!(reopened_audio.channel(0), final_audio.channel(0));
    assert_eq!(reopened_audio.channel(1), final_audio.channel(1));
    assert!(!reopened.editor().is_dirty());
}
