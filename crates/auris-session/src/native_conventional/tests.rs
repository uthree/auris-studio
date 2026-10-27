//! Deterministic render fixtures for native/conventional parity tests.
//!
//! The helpers use only public engine and built-in pack APIs so the parity assertions do
//! not depend on an engine-internal test registry or an installed third-party plugin.

use auris_core::automation::AutomationCurve;
use auris_core::pipeline::content::{
    CapabilityProvider, PluginCapabilities, PluginParameter, Processor, ProcessorKind,
};
use auris_core::pipeline::{AutomationPolicy, ControlDomain, NodeId, PipelineLimits};
use auris_core::project::{AudioSourceBank, Note, Output, Project};
use auris_core::registry::PluginRegistry;
use auris_core::time::Ticks;
use auris_core::{Effect, ParamTarget, PluginKind};
use auris_dsp::{Compressor, DspPack};
use auris_engine::{OfflineOptions, render_project};
use auris_synth::{Chiptune, DrumKit, SynthPack};
use std::collections::BTreeMap;
use std::sync::Arc;

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK_FRAMES: usize = 128;

struct PluginMetadata {
    plugin_id: String,
    kind: PluginKind,
    parameters: Vec<PluginParameter>,
    sidechain: bool,
}

pub(super) struct BuiltinProvider {
    metadata: BTreeMap<NodeId, PluginMetadata>,
}

impl BuiltinProvider {
    /// Test-only capability mutation used to verify policy survives preparation.
    pub(super) fn set_policy_for_node(
        &mut self,
        node: NodeId,
        policy: AutomationPolicy,
    ) -> Option<String> {
        let metadata = self.metadata.get_mut(&node)?;
        for parameter in &mut metadata.parameters {
            parameter.policy = policy;
        }
        metadata
            .parameters
            .first()
            .map(|parameter| parameter.descriptor.key.to_string())
    }
}

impl CapabilityProvider for BuiltinProvider {
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>> {
        let metadata = self.metadata.get(&processor.id)?;
        Some(PluginCapabilities {
            plugin_id: &metadata.plugin_id,
            kind: metadata.kind,
            parameters: &metadata.parameters,
            sidechain: metadata.sidechain,
        })
    }
}

pub(super) fn builtin_provider(
    document: &auris_core::pipeline::document::NativeDocument,
) -> BuiltinProvider {
    let registry = fixture_registry();
    let mut metadata = BTreeMap::new();
    for processor in &document.content.processors {
        let (plugin_id, kind, descriptors, sidechain) = match &processor.kind {
            ProcessorKind::Instrument(instance) => {
                let plugin = registry
                    .create_instrument(&instance.plugin_id)
                    .expect("fixture instrument");
                (
                    instance.plugin_id.clone(),
                    PluginKind::Instrument,
                    plugin.parameters().to_vec(),
                    false,
                )
            }
            ProcessorKind::Effect {
                plugin: instance, ..
            } => {
                let plugin = registry
                    .create_effect(&instance.plugin_id)
                    .expect("fixture effect");
                (
                    instance.plugin_id.clone(),
                    PluginKind::Effect,
                    plugin.parameters().to_vec(),
                    plugin.wants_sidechain(),
                )
            }
            _ => continue,
        };
        let parameters = descriptors
            .into_iter()
            .map(|descriptor| PluginParameter {
                domain: ControlDomain::Native {
                    unit: descriptor.unit,
                    min: descriptor.min,
                    max: descriptor.max,
                },
                descriptor,
                policy: AutomationPolicy::Continuous,
            })
            .collect();
        metadata.insert(
            processor.id,
            PluginMetadata {
                plugin_id,
                kind,
                parameters,
                sidechain,
            },
        );
    }
    BuiltinProvider { metadata }
}

pub(super) fn content_limits() -> auris_core::pipeline::content::ContentLimits {
    auris_core::pipeline::content::ContentLimits {
        sources: 64,
        clips: 1024,
        stages: 4096,
        processors: 4096,
        groups: 256,
        tracks: 256,
        edges: 8192,
        external_ids: 1024,
        automations: 256,
        automation_points: 8192,
        bindings: 1024,
    }
}

pub(super) fn graph_limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 4096,
        edges: 8192,
        ports: 16384,
        parameter_keys: 1024,
        text_bytes: 1 << 20,
    }
}

/// Installs the deterministic built-in instrument/effect packs used by parity fixtures.
pub(super) fn fixture_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>().install::<DspPack>();
    registry
}

/// A short melodic fixture with one deterministic built-in instrument.
pub(super) fn instrument_fixture() -> Project {
    let mut project = Project::new("native parity instrument", SAMPLE_RATE);
    let track = project.add_instrument_track("Instrument", Chiptune::ID);
    let clip = project
        .add_midi_clip(track, "A", Ticks::ZERO, Ticks::from_beats(1.0))
        .expect("fixture clip");
    project
        .midi_clip_mut(clip)
        .expect("fixture clip lookup")
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(1.0)));
    project
}

/// A deterministic drum fixture using the built-in drumkit and a distinct drum track kind.
pub(super) fn drum_fixture() -> Project {
    let mut project = Project::new("native parity drums", SAMPLE_RATE);
    let track = project.add_drum_track("Drums", DrumKit::ID);
    let clip = project
        .add_midi_clip(track, "Kick", Ticks::ZERO, Ticks::from_beats(1.0))
        .expect("fixture clip");
    project
        .midi_clip_mut(clip)
        .expect("fixture clip lookup")
        .notes
        .push(Note::new(36, Ticks::ZERO, Ticks::from_beats(1.0)));
    project
}

/// Renders a conventional project with the same bounded block size used by parity comparisons.
pub(super) fn render_fixture(
    project: &Project,
) -> Result<auris_core::AudioBuffer, auris_engine::EngineError> {
    render_fixture_with_bank(project, &AudioSourceBank::new())
}

fn render_fixture_with_bank(
    project: &Project,
    bank: &AudioSourceBank,
) -> Result<auris_core::AudioBuffer, auris_engine::EngineError> {
    render_project(
        project,
        bank,
        &fixture_registry(),
        &OfflineOptions {
            end_frames: Some(4_096),
            block_frames: BLOCK_FRAMES,
            include_tail: false,
            ..OfflineOptions::default()
        },
    )
}

fn audio_bus_fixture() -> (Project, AudioSourceBank) {
    let mut project = Project::new("native parity audio", SAMPLE_RATE);
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    let source = project.add_audio_source(
        "pulse",
        auris_core::AssetPath::inside("Audio/pulse.wav"),
        256,
        SAMPLE_RATE,
        2,
    );
    project
        .add_audio_clip(audio, source, Ticks::ZERO)
        .expect("audio clip");
    project.track_mut(audio).expect("audio track").outputs = vec![Output::Bus(bus), Output::Master];
    let send_id = project.next_send_id();
    project
        .track_mut(audio)
        .expect("audio track")
        .sends
        .push(auris_core::project::AuxSend::new(send_id, bus));
    let effect_id = Compressor::new().descriptor().id.to_string();
    project.add_effect(Some(bus), effect_id.clone());
    let master_effect = project.add_effect(None, effect_id).expect("master effect");
    project
        .master
        .effects
        .iter_mut()
        .find(|effect| effect.id == master_effect)
        .expect("master effect slot")
        .sidechain = Some(audio);
    let mut bank = AudioSourceBank::new();
    let samples = vec![0.25f32; 256];
    bank.insert(
        source,
        Arc::new(
            auris_core::AudioBuffer::from_planar(vec![samples.clone(), samples], SAMPLE_RATE)
                .expect("audio buffer"),
        ),
    );
    (project, bank)
}

#[test]
fn conventional_lowering_preserves_builtin_instrument_render() {
    let legacy = instrument_fixture();
    let native = auris_io::project_to_native(&legacy, content_limits()).expect("native conversion");
    let provider = builtin_provider(&native);
    let lowered =
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .expect("native lowering");
    let before = render_fixture(&legacy).expect("legacy render");
    let after = render_fixture(&lowered).expect("lowered render");
    assert_eq!(before.frame_count(), after.frame_count());
    assert_eq!(before.sample_rate(), after.sample_rate());
    for channel in 0..2 {
        assert!(
            before
                .channel(channel)
                .iter()
                .any(|sample| sample.abs() > 1.0e-7)
        );
        assert_eq!(before.channel(channel).len(), after.channel(channel).len());
        assert_eq!(before.channel(channel), after.channel(channel));
    }
}

#[test]
fn conventional_lowering_preserves_builtin_drum_render() {
    let legacy = drum_fixture();
    let native = auris_io::project_to_native(&legacy, content_limits()).expect("native conversion");
    let provider = builtin_provider(&native);
    let lowered =
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .expect("native lowering");
    let before = render_fixture(&legacy).expect("legacy render");
    let after = render_fixture(&lowered).expect("lowered render");
    assert_eq!(before.frame_count(), after.frame_count());
    for channel in 0..2 {
        assert!(
            before
                .channel(channel)
                .iter()
                .any(|sample| sample.abs() > 1.0e-7)
        );
        assert_eq!(before.channel(channel), after.channel(channel));
    }
}

#[test]
fn conventional_lowering_preserves_audio_bus_and_send_render() {
    for pre_fader in [false, true] {
        let (mut legacy, bank) = audio_bus_fixture();
        legacy.tracks[0].mixer.gain_db = -6.0;
        legacy.tracks[0].sends[0].pre_fader = pre_fader;
        let native =
            auris_io::project_to_native(&legacy, content_limits()).expect("native conversion");
        let provider = builtin_provider(&native);
        let lowered =
            super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
                .expect("native lowering");
        let before = render_fixture_with_bank(&legacy, &bank).expect("legacy render");
        let after = render_fixture_with_bank(&lowered, &bank).expect("lowered render");
        assert_eq!(before.frame_count(), after.frame_count());
        for channel in 0..2 {
            assert!(
                before
                    .channel(channel)
                    .iter()
                    .any(|sample| sample.abs() > 1.0e-7)
            );
            assert_eq!(before.channel(channel), after.channel(channel));
        }
    }
}

#[test]
fn native_clip_stage_edit_reaches_the_performer_and_retains_authored_notes() {
    let mut legacy = instrument_fixture();
    let clip = legacy.tracks[0].kind.note_clips().unwrap()[0].id;
    legacy
        .midi_clip_mut(clip)
        .unwrap()
        .transforms
        .push(auris_core::NoteTransform::Transpose { semitones: 0 });
    let mut native = auris_io::project_to_native(&legacy, content_limits()).unwrap();
    let auris_core::pipeline::content::SourceContent::Notes(clips) =
        &mut native.content.sources[0].content
    else {
        panic!("note source");
    };
    let notes = clips[0].notes.clone();
    let stage = clips[0].transforms[0].id;
    clips[0].transforms[0].transform = auris_core::NoteTransform::Transpose { semitones: 12 };
    let native_before = native.clone();
    let lowered = super::lower_native_conventional(
        &native,
        &builtin_provider(&native),
        content_limits(),
        graph_limits(),
    )
    .unwrap();
    assert_eq!(native, native_before);
    assert_eq!(lowered.midi_clip(clip).unwrap().1.notes, notes);
    let auris_core::pipeline::content::SourceContent::Notes(clips) =
        &native.content.sources[0].content
    else {
        panic!("note source");
    };
    assert_eq!(clips[0].transforms[0].id, stage);
    legacy.midi_clip_mut(clip).unwrap().transforms[0] =
        auris_core::NoteTransform::Transpose { semitones: 12 };
    let expected = render_fixture(&legacy).unwrap();
    let actual = render_fixture(&lowered).unwrap();
    let original = render_fixture(&instrument_fixture()).unwrap();
    for channel in 0..2 {
        assert_eq!(actual.channel(channel), expected.channel(channel));
    }
    assert_ne!(actual.channel(0), original.channel(0));
}

#[test]
fn native_gain_mute_and_gain_automation_have_numeric_render_effects() {
    let legacy = instrument_fixture();
    let baseline = render_fixture(&legacy).expect("baseline render");
    let track = legacy.tracks[0].id;

    let mut gain_native =
        auris_io::project_to_native(&legacy, content_limits()).expect("gain conversion");
    let gain_group = gain_native
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .expect("gain binding")
        .group;
    let gain_strip = gain_native
        .content
        .groups
        .iter()
        .find(|group| group.id == gain_group)
        .expect("gain group")
        .strip;
    let gain_processor = gain_native
        .content
        .processors
        .iter_mut()
        .find(|processor| processor.id == gain_strip)
        .expect("gain strip");
    let auris_core::pipeline::content::ProcessorKind::Strip(strip) = &mut gain_processor.kind
    else {
        panic!("gain processor");
    };
    strip.gain_db = -12.0;
    let gain_lowered = super::lower_native_conventional(
        &gain_native,
        &builtin_provider(&gain_native),
        content_limits(),
        graph_limits(),
    )
    .expect("gain lowering");
    let gained = render_fixture(&gain_lowered).expect("gain render");
    let baseline_peak = baseline
        .channel(0)
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    let gain_peak = gained
        .channel(0)
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    assert!(
        gain_peak > 0.0 && gain_peak < baseline_peak * 0.4,
        "gain did not attenuate: {baseline_peak} -> {gain_peak}"
    );
    assert!((gain_peak / baseline_peak - 10.0f32.powf(-12.0 / 20.0)).abs() < 1.0e-5);

    let mut mute_native =
        auris_io::project_to_native(&legacy, content_limits()).expect("mute conversion");
    let mute_group = mute_native
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .expect("mute binding")
        .group;
    let mute_strip = mute_native
        .content
        .groups
        .iter()
        .find(|group| group.id == mute_group)
        .expect("mute group")
        .strip;
    let mute_processor = mute_native
        .content
        .processors
        .iter_mut()
        .find(|processor| processor.id == mute_strip)
        .expect("mute strip");
    let auris_core::pipeline::content::ProcessorKind::Strip(strip) = &mut mute_processor.kind
    else {
        panic!("mute processor");
    };
    strip.mute = true;
    let muted = render_fixture(
        &super::lower_native_conventional(
            &mute_native,
            &builtin_provider(&mute_native),
            content_limits(),
            graph_limits(),
        )
        .expect("mute lowering"),
    )
    .expect("mute render");
    for channel in 0..2 {
        assert!(muted.channel(channel).iter().all(|sample| *sample == 0.0));
    }

    let mut automation_project = legacy;
    assert!(automation_project.automation.set_point(
        ParamTarget::TrackGain(track),
        None,
        AutomationCurve::Hold,
        Ticks::ZERO,
        -18.0
    ));
    let automation_native = auris_io::project_to_native(&automation_project, content_limits())
        .expect("automation conversion");
    let automation_lowered = super::lower_native_conventional(
        &automation_native,
        &builtin_provider(&automation_native),
        content_limits(),
        graph_limits(),
    )
    .expect("automation lowering");
    let automated = render_fixture(&automation_lowered).expect("automation render");
    let automated_peak = automated
        .channel(0)
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    assert!(
        automated_peak > 0.0 && automated_peak < baseline_peak * 0.2,
        "gain automation did not attenuate: {baseline_peak} -> {automated_peak}"
    );

    let mut disabled_native = auris_io::project_to_native(&automation_project, content_limits())
        .expect("disabled automation conversion");
    disabled_native.content.automation_bindings[0].enabled = false;
    let disabled_lowered = super::lower_native_conventional(
        &disabled_native,
        &builtin_provider(&disabled_native),
        content_limits(),
        graph_limits(),
    )
    .expect("disabled automation lowering");
    let disabled = render_fixture(&disabled_lowered).expect("disabled automation render");
    assert_eq!(disabled.frame_count(), baseline.frame_count());
    for channel in 0..2 {
        assert_eq!(disabled.channel(channel), baseline.channel(channel));
    }
}

#[test]
fn singer_guide_declared_take_and_missing_take_keep_their_selection() {
    let mut legacy = Project::new("Singer parity", SAMPLE_RATE);
    let track = legacy.add_singer_track("Singer", Chiptune::ID);
    let clip = legacy
        .add_midi_clip(track, "Lyrics", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    legacy
        .midi_clip_mut(clip)
        .unwrap()
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    let guide = render_fixture(&legacy).unwrap();
    assert!(guide.channel(0).iter().any(|sample| sample.abs() > 1.0e-7));
    let native = auris_io::project_to_native(&legacy, content_limits()).unwrap();
    let lowered = super::lower_native_conventional(
        &native,
        &builtin_provider(&native),
        content_limits(),
        graph_limits(),
    )
    .unwrap();
    let actual = render_fixture(&lowered).unwrap();
    for channel in 0..2 {
        assert_eq!(guide.channel(channel), actual.channel(channel));
    }

    let source = legacy.add_audio_source(
        "Take",
        auris_core::AssetPath::inside("Audio/take.wav"),
        4_096,
        SAMPLE_RATE,
        2,
    );
    legacy
        .track_mut(track)
        .unwrap()
        .kind
        .as_singer_mut()
        .unwrap()
        .take = Some(auris_core::SingerTake {
        source,
        fingerprint: 123,
        seed: 42,
        provenance: None,
        backend_pitch: None,
    });
    let mut bank = AudioSourceBank::new();
    bank.insert(
        source,
        Arc::new(
            auris_core::AudioBuffer::from_planar(
                vec![vec![0.2; 4_096], vec![-0.1; 4_096]],
                SAMPLE_RATE,
            )
            .unwrap(),
        ),
    );
    let native = auris_io::project_to_native(&legacy, content_limits()).unwrap();
    let lowered = super::lower_native_conventional(
        &native,
        &builtin_provider(&native),
        content_limits(),
        graph_limits(),
    )
    .unwrap();
    let expected = render_fixture_with_bank(&legacy, &bank).unwrap();
    let actual = render_fixture_with_bank(&lowered, &bank).unwrap();
    for channel in 0..2 {
        assert_eq!(expected.channel(channel), actual.channel(channel));
        assert!(
            actual
                .channel(channel)
                .iter()
                .any(|sample| sample.abs() > 1.0e-7)
        );
    }
    assert_ne!(actual.channel(0), guide.channel(0));
    let missing = render_fixture(&lowered).unwrap();
    for channel in 0..2 {
        assert!(missing.channel(channel).iter().all(|sample| *sample == 0.0));
    }
}

#[test]
fn plugin_automation_resolves_domains_and_runtime_positions_by_stable_key() {
    let mut legacy = instrument_fixture();
    let track = legacy.tracks[0].id;
    let plugin = fixture_registry().create_instrument(Chiptune::ID).unwrap();
    let descriptor = &plugin.parameters()[0];
    let key = descriptor.key.to_string();
    let value = descriptor.default;
    assert!(legacy.automation.set_point(
        ParamTarget::Instrument {
            track,
            param: descriptor.id
        },
        Some(key.clone()),
        AutomationCurve::Hold,
        Ticks::ZERO,
        value,
    ));
    let mut native = auris_io::project_to_native(&legacy, content_limits()).unwrap();
    let mut provider = builtin_provider(&native);
    assert!(
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .is_err()
    );
    let target_node = native.content.automation_bindings[0].target.node;
    let metadata = provider.metadata.get_mut(&target_node).unwrap();
    metadata.parameters.swap(0, 1);
    metadata.parameters[0].descriptor.id = auris_core::ParamId(0);
    metadata.parameters[1].descriptor.id = auris_core::ParamId(1);
    native.content.automation[0]
        .resolve_domain(metadata.parameters[1].domain)
        .unwrap();
    let before = native.clone();
    let lowered =
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .unwrap();
    assert_eq!(native, before);
    let lane = &lowered.automation.lanes()[0];
    assert_eq!(
        lane.target,
        ParamTarget::Instrument {
            track,
            param: auris_core::ParamId(1)
        }
    );
    assert_eq!(lane.key(), Some(key.as_str()));
    assert_eq!(lane.value_at(Ticks::ZERO), value);
    provider.metadata.get_mut(&target_node).unwrap().parameters[1].policy =
        AutomationPolicy::EventSampled;
    native
        .content
        .resolve_pipeline(
            &native.external_ids(),
            content_limits(),
            graph_limits(),
            &provider,
        )
        .unwrap();
    assert!(matches!(
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits()),
        Err(super::NativeConventionalError::Unsupported(_)),
    ));
    assert_eq!(native, before);
    native.content.automation_bindings[0].enabled = false;
    let disabled =
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .unwrap();
    assert!(disabled.automation.is_empty());
    native.content.automation_bindings[0].enabled = true;
    provider.metadata.get_mut(&target_node).unwrap().parameters[1].policy =
        AutomationPolicy::Continuous;
    native.content.automation_bindings[0].target.key = "missing-key".into();
    let before = native.clone();
    assert!(
        super::lower_native_conventional(&native, &provider, content_limits(), graph_limits())
            .is_err()
    );
    assert_eq!(native, before);
}
