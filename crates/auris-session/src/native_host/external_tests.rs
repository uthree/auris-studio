//! Opt-in native-host integration coverage for an installed instrument.
//!
//! Set AURIS_NATIVE_PLUGIN and AURIS_NATIVE_CONTROL to a CLAP/VST3 instrument and volume
//! control. AURIS_NATIVE_HOST_GUI and AURIS_NATIVE_HOST_DEVICE opt into windows and hardware.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_core::AssetPath;
use auris_core::AudioBuffer;
use auris_core::pipeline::content::{
    CapabilityProvider, ContentLimits, PluginCapabilities, PluginParameter, Processor,
    ProcessorKind,
};
use auris_core::pipeline::{AutomationPolicy, ControlDomain, NodeId, PipelineLimits};
use auris_core::plugin::{PluginKind, PrepareContext};
use auris_core::project::{AudioSourceBank, Note, Project};
use auris_core::time::Ticks;
use auris_engine::{RenderGraph, Transport, render_block};
use auris_vst3::plugins_in;

use super::{NativeHostResources, NativeHostedPlayback, Prepared};

struct Metadata {
    plugin_id: String,
    kind: PluginKind,
    parameters: Vec<PluginParameter>,
    sidechain: bool,
}

struct Provider {
    metadata: BTreeMap<NodeId, Metadata>,
}

impl CapabilityProvider for Provider {
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

fn limits() -> (ContentLimits, PipelineLimits) {
    (
        ContentLimits {
            sources: 8,
            clips: 32,
            stages: 64,
            processors: 128,
            groups: 16,
            tracks: 8,
            edges: 256,
            external_ids: 16,
            automations: 16,
            automation_points: 128,
            bindings: 32,
        },
        PipelineLimits {
            nodes: 128,
            edges: 256,
            ports: 32_768,
            parameter_keys: 8_192,
            text_bytes: 1 << 20,
        },
    )
}

fn plugin_path() -> Option<PathBuf> {
    std::env::var_os("AURIS_NATIVE_PLUGIN").map(PathBuf::from)
}

fn class_id(path: &Path) -> Option<String> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("clap"))
    {
        // SAFETY: This opt-in test explicitly receives the installed plugin path from its caller.
        let library = unsafe { auris_clap::ClapLibrary::load(path) }.expect("CLAP library");
        return library
            .plugins()
            .expect("CLAP catalog")
            .into_iter()
            .find(|info| info.kind == PluginKind::Instrument)
            .map(|info| format!("{}{}", auris_clap::ID_PREFIX, info.clap_id));
    }
    plugins_in(path)
        .ok()?
        .into_iter()
        .find(|info| matches!(info.kind, PluginKind::Instrument))
        .map(|info| format!("{}{}", auris_vst3::ID_PREFIX, info.class_id))
}

fn provider_for(
    document: &auris_core::pipeline::document::NativeDocument,
    path: &Path,
    prepare: &PrepareContext,
) -> Option<Provider> {
    let mut metadata = BTreeMap::new();
    for processor in &document.content.processors {
        let (instance, kind) = match &processor.kind {
            ProcessorKind::Instrument(instance) => (instance, PluginKind::Instrument),
            ProcessorKind::Effect { plugin, .. } => (plugin, PluginKind::Effect),
            _ => continue,
        };
        let (host, half) =
            super::load_host(path, instance, prepare, kind == PluginKind::Instrument)
                .expect("metadata host");
        let (descriptors, sidechain) = match &host {
            super::Host::Clap(host) => (host.parameters(), host.supports_sidechain()),
            super::Host::Vst3(host) => (host.parameters(), host.supports_sidechain()),
        };
        let parameters = descriptors
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
        metadata.insert(
            processor.id,
            Metadata {
                plugin_id: instance.plugin_id.clone(),
                kind,
                parameters,
                sidechain,
            },
        );
        drop(half);
    }
    Some(Provider { metadata })
}

fn render_graph(graph: &mut RenderGraph) -> auris_core::AudioBuffer {
    let mut output = AudioBuffer::new(2, 4096, 48_000.0);
    render_block(graph, &mut Transport::playing_from(0), &mut output, true);
    output
}

fn rms(buffer: &auris_core::AudioBuffer) -> f64 {
    let (sum, count) = (0..buffer.channel_count())
        .flat_map(|channel| buffer.channel(channel).iter().copied())
        .fold((0.0_f64, 0_u64), |(sum, count), sample| {
            (sum + f64::from(sample).powi(2), count + 1)
        });
    (sum / count.max(1) as f64).sqrt()
}

#[cfg(target_os = "windows")]
fn editor_windows() -> Option<Vec<usize>> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetClassNameW};
    use windows::core::BOOL;
    unsafe extern "system" fn collect(hwnd: HWND, context: LPARAM) -> BOOL {
        let mut class = [0_u16; 64];
        // SAFETY: Windows supplies a live handle and this buffer is valid for the call.
        let length = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if String::from_utf16_lossy(&class[..length]) == "VST3PluginWindow" {
            // SAFETY: The synchronous enumeration owns this borrowed vector until it returns.
            unsafe { &mut *(context.0 as *mut Vec<usize>) }.push(hwnd.0 as usize);
        }
        true.into()
    }
    let mut windows = Vec::<usize>::new();
    // SAFETY: Only enumerate this test thread's windows; the callback cannot outlive the vector.
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect),
            LPARAM(&mut windows as *mut Vec<usize> as isize),
        )
    }
    .ok()
    .expect("enumerate native editors");
    Some(windows)
}

#[cfg(not(target_os = "windows"))]
fn editor_windows() -> Option<Vec<usize>> {
    None
}

#[test]
#[ignore = "loads and executes an installed third-party instrument"]
fn native_hosted_candidate_is_isolated_and_renders() {
    let path = plugin_path().expect("AURIS_NATIVE_PLUGIN must name a plugin");
    let plugin_id = class_id(&path).expect("plugin must expose an instrument class");
    let prepare = PrepareContext::new(48_000.0, 256, 2);
    let mut project = Project::new("native host integration", 48_000.0);
    let track = project.add_instrument_track("Hosted", &plugin_id);
    assert!(project.set_hosted_instrument(track, &plugin_id, AssetPath::external(&path)));
    let clip = project
        .add_midi_clip(track, "note", Ticks::ZERO, Ticks::from_beats(1.0))
        .expect("fixture clip");
    project
        .midi_clip_mut(clip)
        .expect("fixture clip")
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(1.0)));

    let document = auris_io::project_to_native(&project, limits().0).expect("native conversion");
    let provider = provider_for(&document, &path, &prepare).expect("capabilities");
    let settings = auris_engine::AudioSettings::default();
    let plugin_paths = [path.clone()];
    let device = std::env::var_os("AURIS_NATIVE_HOST_DEVICE").is_some();
    let mut playback = if device {
        NativeHostedPlayback::start(&settings, &plugin_paths).expect("real output device")
    } else {
        NativeHostedPlayback::silent(&settings, &plugin_paths)
    };
    let (content_limits, graph_limits) = limits();
    playback
        .prepare(
            &document,
            1,
            &provider,
            NativeHostResources {
                bank: &AudioSourceBank::new(),
                registry: &auris_core::registry::PluginRegistry::new(),
                project_folder: None,
                content_limits,
                graph_limits,
            },
        )
        .expect("native host preparation");
    let Prepared {
        graph: first_graph,
        owners: first_owners,
    } = playback.prepared.take().expect("prepared native graph");
    let mut first_graph = first_graph;
    let first = render_graph(&mut first_graph);
    assert!(rms(&first).is_finite() && rms(&first) > 1.0e-6);
    let node = *first_owners.hosts.keys().next().expect("hosted node");
    let first_generation = first_owners.generation;
    playback.pending = Some(first_owners);
    playback.promote(Some(first_generation), &mut Default::default());
    let gui = std::env::var_os("AURIS_NATIVE_HOST_GUI").is_some();
    if gui {
        playback
            .set_window_open(node, true)
            .expect("first native editor");
    }
    let original_windows = gui.then(editor_windows).flatten();
    if let Some(windows) = &original_windows {
        assert_eq!(windows.len(), 1);
    }
    let snapshot_before = playback.snapshot(node).expect("published snapshot").state;

    let mut changed = document.clone();
    let processor = changed
        .content
        .processors
        .iter_mut()
        .find_map(|processor| match &mut processor.kind {
            ProcessorKind::Instrument(instance) => Some(instance),
            _ => None,
        })
        .expect("instrument processor");
    let requested_key =
        std::env::var("AURIS_NATIVE_CONTROL").expect("name the instrument volume control");
    let descriptor = provider.metadata[&node]
        .parameters
        .iter()
        .map(|parameter| &parameter.descriptor)
        .find(|parameter| {
            (parameter.key.eq_ignore_ascii_case(&requested_key)
                || parameter.name.eq_ignore_ascii_case(&requested_key))
                && !parameter.read_only
                && parameter.min < parameter.max
        })
        .expect("requested writable volume control");
    processor
        .state
        .params
        .insert(descriptor.key.to_string(), descriptor.min);
    let provider_changed = provider_for(&changed, &path, &prepare).expect("changed capabilities");
    playback
        .prepare(
            &changed,
            2,
            &provider_changed,
            NativeHostResources {
                bank: &AudioSourceBank::new(),
                registry: &auris_core::registry::PluginRegistry::new(),
                project_folder: None,
                content_limits,
                graph_limits,
            },
        )
        .expect("changed native host preparation");
    let Prepared {
        graph: second_graph,
        owners: second_owners,
    } = playback.prepared.take().expect("changed native graph");
    let mut second_graph = second_graph;
    let second = render_graph(&mut second_graph);
    assert!(rms(&second).is_finite());
    assert!(
        rms(&second) < rms(&first) * 0.5,
        "volume must reduce actual audio"
    );
    assert_eq!(
        playback.snapshot(node).expect("old host snapshot").state,
        snapshot_before
    );
    if gui {
        assert!(
            playback.window_is_open(node),
            "preparation keeps old editor"
        );
        assert_eq!(
            editor_windows(),
            original_windows,
            "preparation preserves the real editor"
        );
    }
    let second_generation = second_owners.generation;
    playback.pending = Some(second_owners);
    let mut update = super::NativeHostUpdate::default();
    playback.promote(Some(second_generation), &mut update);
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert_eq!(
        playback.window_node(crate::PluginWindow::Instrument(track)),
        Some(node)
    );
    if gui {
        assert!(
            playback.window_is_open(node),
            "adoption transfers editor intent"
        );
        if let Some(windows) = editor_windows() {
            assert_eq!(windows.len(), 1);
        }
        playback
            .set_window_open(node, false)
            .expect("close native editor");
        if let Some(windows) = editor_windows() {
            assert!(windows.is_empty(), "host must destroy the native window");
        }
    }
    assert_eq!(playback.retiring.len(), 1);
    drop(first_graph);
    let update = playback.poll();
    assert!(update.errors.is_empty(), "{:?}", update.errors);
    assert!(playback.retiring.is_empty());
    assert_eq!(playback.snapshot(node).expect("new host").revision, 2);
    drop(second_graph);
    if device {
        let third = playback
            .prepare(
                &changed,
                3,
                &provider_changed,
                NativeHostResources {
                    bank: &AudioSourceBank::new(),
                    registry: &auris_core::registry::PluginRegistry::new(),
                    project_folder: None,
                    content_limits,
                    graph_limits,
                },
            )
            .expect("real callback candidate");
        playback
            .queue_prepared(3)
            .expect("enqueue native hosted graph");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while playback.published_generation() != Some(third) {
            let update = playback.poll();
            assert!(update.errors.is_empty(), "{:?}", update.errors);
            assert!(
                std::time::Instant::now() < deadline,
                "callback adoption timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            playback
                .snapshot(node)
                .expect("callback-published host")
                .revision,
            3
        );
        assert!(playback.retiring.is_empty());
    }
    eprintln!(
        "native hosted RMS: {} -> {}; GUI={gui}; device={device}",
        rms(&first),
        rms(&second)
    );
}
