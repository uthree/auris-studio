use std::collections::BTreeMap;
use std::path::Path;

use auris_clap::testkit::{FIXTURE_ID, fixture_library};
use auris_core::pipeline::content::{
    CapabilityProvider, ContentLimits, PluginCapabilities, PluginParameter, Processor,
    ProcessorKind,
};
use auris_core::pipeline::{AutomationPolicy, ControlDomain, NodeId, PipelineLimits};
use auris_core::plugin::{Effect, PluginKind, PrepareContext};
use auris_core::project::{AudioSourceBank, Project};
use auris_core::{AssetPath, PluginRegistry};
use auris_engine::native::{
    NativeRenderInput, NativeRenderTarget, NodePlacedEffects, NodePlacedInstruments,
};
use auris_engine::{AudioSettings, RenderGraph};

use super::{
    HostGeneration, NativeHostError, NativeHostResources, NativeHostUpdate, NativeHostedPlayback,
    Prepared,
};

struct FixtureProvider {
    node: NodeId,
    plugin_id: String,
    parameters: Vec<PluginParameter>,
    sidechain: bool,
}

impl CapabilityProvider for FixtureProvider {
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>> {
        (processor.id == self.node).then_some(PluginCapabilities {
            plugin_id: &self.plugin_id,
            kind: PluginKind::Effect,
            parameters: &self.parameters,
            sidechain: self.sidechain,
        })
    }
}

fn limits() -> (ContentLimits, PipelineLimits) {
    (
        ContentLimits {
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
        },
        PipelineLimits {
            nodes: 4096,
            edges: 8192,
            ports: 16384,
            parameter_keys: 1024,
            text_bytes: 1 << 20,
        },
    )
}

fn hosted_document(
    path: &Path,
) -> (
    auris_core::pipeline::document::NativeDocument,
    FixtureProvider,
    NodeId,
) {
    let mut project = Project::new("native clap", 48_000.0);
    project
        .add_hosted_effect(
            None,
            format!("clap:{FIXTURE_ID}"),
            AssetPath::external(path),
        )
        .expect("master effect");
    let (content_limits, _) = limits();
    let document = auris_io::project_to_native(&project, content_limits).expect("native document");
    let processor = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Effect { .. }))
        .expect("hosted processor");
    let node = processor.id;
    let library = fixture_library();
    let mut plugin = library.instantiate(FIXTURE_ID).expect("fixture plugin");
    let parameters = plugin
        .parameters()
        .iter()
        .cloned()
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
    let effect = plugin
        .activate(&PrepareContext::new(48_000.0, 256, 2))
        .expect("fixture effect");
    let sidechain = effect.wants_sidechain();
    drop(effect);
    plugin.release();
    (
        document,
        FixtureProvider {
            node,
            plugin_id: format!("clap:{FIXTURE_ID}"),
            parameters,
            sidechain,
        },
        node,
    )
}

fn prepare_fixture(
    playback: &mut NativeHostedPlayback,
    document: &auris_core::pipeline::document::NativeDocument,
    provider: &FixtureProvider,
    revision: u64,
) -> Result<auris_engine::native::NativeGraphGeneration, NativeHostError> {
    let (content_limits, graph_limits) = limits();
    let bank = AudioSourceBank::new();
    let registry = PluginRegistry::new();
    let resources = NativeHostResources {
        bank: &bank,
        registry: &registry,
        project_folder: None,
        content_limits,
        graph_limits,
    };
    playback.prepare_with(
        document,
        revision,
        provider,
        resources,
        |_file, instance, prepare, instrument| {
            let (host, half) = super::clap::ClapHost::load_from_library(
                fixture_library(),
                &instance.plugin_id,
                &instance.state,
                prepare,
                instrument,
            )?;
            Ok((super::Host::Clap(host), half))
        },
    )
}

fn native_graph() -> Box<RenderGraph> {
    let input = NativeRenderInput::new(
        Project::default(),
        vec![(NodeId(1), NativeRenderTarget::Strip(None))],
        BTreeMap::new(),
    )
    .expect("native input");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    Box::new(
        input
            .build(
                &AudioSourceBank::new(),
                &auris_core::registry::PluginRegistry::new(),
                &mut effects,
                &mut instruments,
                256,
                48_000.0,
            )
            .expect("native graph"),
    )
}

fn prepared_empty() -> Prepared {
    let graph = native_graph();
    let generation = graph.native_generation().expect("native generation");
    Prepared {
        graph: *graph,
        owners: HostGeneration {
            generation,
            revision: 7,
            hosts: BTreeMap::new(),
            windows: BTreeMap::new(),
        },
    }
}

#[test]
fn silent_engine_rejects_prepared_candidate_without_consuming_it() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let prepared = prepared_empty();
    let generation = prepared.owners.generation;
    playback.prepared = Some(prepared);

    assert!(playback.queue_prepared(7).is_err());
    assert_eq!(playback.prepared_generation(), Some(generation));
}

#[test]
fn changed_document_revision_cannot_enqueue_the_old_candidate() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let prepared = prepared_empty();
    let generation = prepared.owners.generation;
    playback.prepared = Some(prepared);
    assert!(matches!(
        playback.queue_prepared(8),
        Err(NativeHostError::StaleRevision {
            prepared: 7,
            current: 8
        })
    ));
    assert_eq!(playback.prepared_generation(), Some(generation));
    assert!(playback.pending_generation().is_none());
}

#[test]
fn unmatched_adoption_does_not_promote_pending_hosts() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let generation = prepared_empty().owners.generation;
    playback.pending = Some(HostGeneration {
        generation,
        revision: 9,
        hosts: BTreeMap::new(),
        windows: BTreeMap::new(),
    });
    let mut update = NativeHostUpdate::default();

    playback.promote(None, &mut update);

    assert_eq!(playback.pending_generation(), Some(generation));
    assert_eq!(playback.published_generation(), None);
    assert_eq!(update.adopted, None);
}

#[test]
fn exact_adoption_promotes_only_the_matching_generation() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let generation = prepared_empty().owners.generation;
    playback.pending = Some(HostGeneration {
        generation,
        revision: 11,
        hosts: BTreeMap::new(),
        windows: BTreeMap::new(),
    });
    let mut update = NativeHostUpdate::default();

    playback.promote(Some(generation), &mut update);

    assert_eq!(playback.pending_generation(), None);
    assert_eq!(playback.published_generation(), Some(generation));
    assert_eq!(update.adopted, Some(generation));
}

#[test]
fn lost_output_reports_once_and_keeps_published_hosts() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let prepared = prepared_empty();
    let generation = prepared.owners.generation;
    playback.published = Some(prepared.owners);
    playback.output_was_running = true;
    assert!(matches!(
        playback.poll().errors.as_slice(),
        [NativeHostError::Engine(
            auris_engine::EngineError::NotRunning
        )]
    ));
    assert!(playback.poll().errors.is_empty());
    assert_eq!(playback.published_generation(), Some(generation));
}

#[test]
fn stopped_device_retires_unadopted_candidate_without_promoting_it() {
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    playback.pending = Some(prepared_empty().owners);
    assert!(!playback.poll().errors.is_empty());
    assert!(playback.pending_generation().is_none());
    assert!(playback.published_generation().is_none());
}

#[test]
fn untrusted_host_is_rejected_before_the_loader_runs() {
    let directory = tempfile::tempdir().expect("untrusted directory");
    let file = directory.path().join("fixture.clap");
    std::fs::write(&file, b"fixture").expect("fixture marker");
    let (document, provider, node) = hosted_document(&file);
    let mut playback = NativeHostedPlayback::silent(&AudioSettings::default(), &[]);
    let (content_limits, graph_limits) = limits();
    let result = playback.prepare_with(
        &document,
        1,
        &provider,
        NativeHostResources {
            bank: &AudioSourceBank::new(),
            registry: &PluginRegistry::new(),
            project_folder: None,
            content_limits,
            graph_limits,
        },
        |_, _, _, _| panic!("untrusted code must never be loaded"),
    );
    assert!(matches!(result, Err(NativeHostError::UntrustedPlugin(id)) if id == node));
}

#[test]
fn hosted_candidate_promotes_editor_intent_and_retires_old_instance_after_graph_return() {
    let directory = tempfile::tempdir().expect("plugin directory");
    let file = directory.path().join("fixture.clap");
    std::fs::write(&file, b"fixture").expect("fixture marker");
    let (document, provider, node) = hosted_document(&file);
    let mut playback =
        NativeHostedPlayback::silent(&AudioSettings::default(), &[directory.path().to_path_buf()]);

    let first = prepare_fixture(&mut playback, &document, &provider, 3).expect("first candidate");
    let first_prepared = playback.prepared.take().expect("prepared first");
    let first_graph = first_prepared.graph;
    playback.pending = Some(first_prepared.owners);
    let mut update = NativeHostUpdate::default();
    playback.promote(Some(first), &mut update);
    assert_eq!(playback.published_generation(), Some(first));

    playback.open_windows.insert(node);
    playback
        .set_window_open(node, true)
        .expect("open first editor");
    assert!(playback.window_is_open(node));

    let second =
        prepare_fixture(&mut playback, &document, &provider, 4).expect("replacement candidate");
    let second_prepared = playback.prepared.take().expect("prepared second");
    let _second_graph = second_prepared.graph;
    playback.pending = Some(second_prepared.owners);
    playback.promote(Some(second), &mut update);
    assert_eq!(playback.published_generation(), Some(second));
    assert!(
        playback.window_is_open(node),
        "editor intent follows adoption"
    );
    let snapshot = playback.snapshot(node).expect("snapshot");
    assert_eq!(snapshot.revision, 4);
    assert_eq!(snapshot.generation, second);
    assert_eq!(playback.retiring.len(), 1, "old host remains parked");

    drop(first_graph);
    playback.poll();
    assert!(
        playback.retiring.is_empty(),
        "old host releases after graph return"
    );
}

#[test]
fn invalid_host_replacement_preserves_the_published_generation() {
    let directory = tempfile::tempdir().expect("plugin directory");
    let file = directory.path().join("fixture.clap");
    std::fs::write(&file, b"fixture").expect("fixture marker");
    let (document, provider, _node) = hosted_document(&file);
    let mut playback =
        NativeHostedPlayback::silent(&AudioSettings::default(), &[directory.path().to_path_buf()]);
    let generation = prepare_fixture(&mut playback, &document, &provider, 8).expect("candidate");
    let prepared = playback.prepared.take().expect("prepared");
    let _graph = prepared.graph;
    playback.pending = Some(prepared.owners);
    playback.promote(Some(generation), &mut NativeHostUpdate::default());

    let invalid = FixtureProvider {
        node: provider.node,
        plugin_id: provider.plugin_id.clone(),
        parameters: provider.parameters.clone(),
        sidechain: !provider.sidechain,
    };
    let error = prepare_fixture(&mut playback, &document, &invalid, 9)
        .expect_err("changed sidechain must reject");
    assert!(matches!(error, NativeHostError::Node { .. }));
    assert_eq!(playback.published_generation(), Some(generation));
    assert_eq!(playback.prepared_generation(), None);
}
